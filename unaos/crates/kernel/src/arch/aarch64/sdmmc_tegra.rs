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
    // ORIN-SDMMC-2: when the write ARM is compiled in on a virt build, one honest metal-only line — the
    // paranoia ladder touches a real Tegra234 SDMMC controller QEMU does not model, so there is nothing to
    // write here and we do zero MMIO. Mirrors the census witness above.
    #[cfg(feature = "sdmmc_arm")]
    serial_println!(
        "{} ORIN-SDMMC-2 write ladder ARMED (UNAOS_SDMMC_ARM=1) but metal-only — no Tegra234 SDMMC on this build (QEMU virt); zero card writes here ::",
        PS
    );
    // ORIN-INSTALL-2: when the third destructive gate is compiled in on a virt build, one honest
    // metal-only line — the self-clone install drives a real Tegra234 SDMMC controller QEMU does not
    // model (and needs the USB boot stick as a block device), so there is nothing to install here and we
    // do zero MMIO. The engine itself is proven on x86 (the UNAOS_INSTALLDEMO witness); the full SD flow's
    // first execution is the attended Orin sitting.
    #[cfg(feature = "install_target")]
    serial_println!(
        "{} ORIN-INSTALL-2 third gate (UNAOS_INSTALL_TARGET_SD=1) compiled-present but metal-only — no Tegra234 SDMMC on this build (QEMU virt); no install here ::",
        PS
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// The metal recon (`sdmmc` + `tegra`) — DTB census (M1), SDHCI identification (M2), sector-0 read (M3).
// ══════════════════════════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "tegra")]
pub use metal::sdmmc_census;

// ORIN-INSTALL-2: the deferred self-clone install entry, called from the boot sequence AFTER the JB2b
// pump has enumerated the USB boot stick as a block device (see the module install section header).
#[cfg(all(feature = "tegra", feature = "install_target"))]
pub use metal::sdmmc_install_from_usb;

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
    #[cfg(feature = "sdmmc_arm")]
    const INT_WRITE_RDY: u32 = 1 << 4; // Buffer Write Ready (host may push the PIO FIFO)
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
    // INSTALL-2: `Clone`+`Copy` under `install_target` so the census can stash the identity into
    // `PENDING_INSTALL` and the deferred post-USB install site can consume it. `cfg_attr`-gated so a
    // plain `sdmmc`/`sdmmc_arm` build carries no derive and stays byte-identical to the merged recon.
    #[cfg_attr(feature = "install_target", derive(Clone, Copy))]
    struct Card {
        block_addressing: bool, // ccs: true = SDHC/SDXC block addressing, false = SDSC byte
        num_blocks: u64,
        csd_version: u8,
        // The raw CMD2 R2 response, retained so the installer can re-announce the identity in its
        // about-to-destroy line. `install_target`-gated so a plain `sdmmc`/`sdmmc_arm` build stores no
        // extra field and stays byte-identical to the merged rung-1/rung-2 recon.
        #[cfg(feature = "install_target")]
        cid: [u32; 4],
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

        Some(Card {
            block_addressing,
            num_blocks,
            csd_version,
            #[cfg(feature = "install_target")]
            cid,
        })
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

    // ══════════════════════════════════════════════════════════════════════════════════════════════════
    // ORIN-SDMMC-2 — the WRITE path behind the paranoia ladder (`sdmmc_arm`-gated: EVERY line below this
    // marker is compiled out unless UNAOS_SDMMC_ARM=1, keeping a plain `sdmmc` build byte-identical to the
    // merged ORIN-SDMMC-1 recon). Law: THE SEATED CARD IS SACRED — no card write happens without BOTH the
    // `sdmmc` feature AND this explicit arm, and even armed, the ladder only ever writes a scratch region
    // that it stashed first and restores after, verifying every step.
    // ══════════════════════════════════════════════════════════════════════════════════════════════════

    /// Single scratch block — the witness writes ONE block (CMD24) this arc (multi-block CMD25 is not used).
    #[cfg(feature = "sdmmc_arm")]
    const SCRATCH_BLOCKS: u64 = 1;

    /// Read one arbitrary block `lba` via polled single-block CMD17 (the generalised `read_sector0`, kept
    /// arm-gated so the rung-1 read path is untouched). READ-ONLY. Returns whether the block was read.
    #[cfg(feature = "sdmmc_arm")]
    fn read_block_at(base: u64, card: &Card, lba: u64, buf: &mut [u8; 512]) -> bool {
        let arg = if card.block_addressing { lba as u32 } else { (lba * 512) as u32 };
        write32(base, INTERRUPT, 0xffff_ffff);
        write32(base, BLKSIZECNT, (1 << 16) | 512);
        if send_command(
            base,
            cmd(17) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK | CMD_ISDATA | CMD_DAT_DIR_READ,
            arg,
        )
        .is_err()
        {
            serial_println!("{}   ladder: CMD17 (READ LBA {}) failed at the link layer ::", PS, lba);
            return false;
        }
        let r1 = read32(base, RESP0);
        if r1 & R1_ERROR_MASK != 0 {
            serial_println!("{}   ladder: CMD17 (LBA {}) R1 error status {:#010x} ::", PS, lba, r1);
            return false;
        }
        if !wait_set(base, INTERRUPT, INT_READ_RDY, DATA_TIMEOUT_MS) {
            serial_println!("{}   ladder: read buffer never became ready (LBA {}) ::", PS, lba);
            return false;
        }
        write32(base, INTERRUPT, INT_READ_RDY);
        for i in 0..128usize {
            let word = read32(base, DATA);
            let off = i * 4;
            buf[off..off + 4].copy_from_slice(&word.to_le_bytes());
        }
        if !wait_set(base, INTERRUPT, INT_DATA_DONE | INT_ERR_ANY, DATA_TIMEOUT_MS) {
            serial_println!("{}   ladder: transfer-complete timeout reading LBA {} ::", PS, lba);
            return false;
        }
        let int = read32(base, INTERRUPT);
        write32(base, INTERRUPT, int);
        if int & INT_ERR_ANY != 0 {
            serial_println!("{}   ladder: data error {:#010x} reading LBA {} ::", PS, int, lba);
            return false;
        }
        true
    }

    /// Dump a 512-byte buffer as 32 rows of 16 hex bytes (offset-labelled) so a stashed original is never
    /// silently lost when a restore step fails. No allocation — a stack line buffer per row.
    #[cfg(feature = "sdmmc_arm")]
    fn dump_hex(buf: &[u8; 512]) {
        const HEXD: &[u8; 16] = b"0123456789abcdef";
        for row in 0..32usize {
            let mut line = [0u8; 16 * 3];
            let mut p = 0usize;
            for col in 0..16usize {
                let b = buf[row * 16 + col];
                line[p] = HEXD[(b >> 4) as usize];
                line[p + 1] = HEXD[(b & 0xf) as usize];
                line[p + 2] = b' ';
                p += 3;
            }
            serial_println!(
                "{}   stash[{:03x}]: {} ::",
                PS,
                row * 16,
                core::str::from_utf8(&line[..p]).unwrap_or("?")
            );
        }
    }

    /// Write one arbitrary block `lba` via polled single-block CMD24 (WRITE_SINGLE_BLOCK). Host->card
    /// direction (DAT_DIR clear). Returns whether the block was written AND the card left the programming
    /// (busy) state cleanly. The ONLY card-write primitive in the driver — reachable only from the ladder,
    /// only under `sdmmc_arm`, only against a stashed scratch region.
    #[cfg(feature = "sdmmc_arm")]
    fn write_block_at(base: u64, card: &Card, lba: u64, buf: &[u8; 512]) -> bool {
        let arg = if card.block_addressing { lba as u32 } else { (lba * 512) as u32 };
        write32(base, INTERRUPT, 0xffff_ffff);
        write32(base, BLKSIZECNT, (1 << 16) | 512);
        // CMD24 WRITE_SINGLE_BLOCK: R1 + data present, host->card (DAT_DIR_READ clear).
        if send_command(
            base,
            cmd(24) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK | CMD_ISDATA,
            arg,
        )
        .is_err()
        {
            serial_println!("{}   ladder: CMD24 (WRITE LBA {}) failed at the link layer ::", PS, lba);
            return false;
        }
        let r1 = read32(base, RESP0);
        if r1 & R1_ERROR_MASK != 0 {
            serial_println!("{}   ladder: CMD24 (LBA {}) R1 error status {:#010x} ::", PS, lba, r1);
            return false;
        }
        if !wait_set(base, INTERRUPT, INT_WRITE_RDY, DATA_TIMEOUT_MS) {
            serial_println!("{}   ladder: write buffer never became ready (LBA {}) ::", PS, lba);
            return false;
        }
        write32(base, INTERRUPT, INT_WRITE_RDY);
        for i in 0..128usize {
            let off = i * 4;
            let word = u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
            write32(base, DATA, word);
        }
        // Transfer-complete: the card has accepted the block off the FIFO.
        if !wait_set(base, INTERRUPT, INT_DATA_DONE | INT_ERR_ANY, DATA_TIMEOUT_MS) {
            serial_println!("{}   ladder: transfer-complete timeout writing LBA {} ::", PS, lba);
            return false;
        }
        let int = read32(base, INTERRUPT);
        write32(base, INTERRUPT, int);
        if int & INT_ERR_ANY != 0 {
            serial_println!("{}   ladder: data error {:#010x} writing LBA {} ::", PS, int, lba);
            return false;
        }
        // The card asserts DAT0 busy while it programs the flash; wait for it to release before we treat the
        // write as durable (a following read would otherwise race the internal programming). `send_command`
        // already waits DAT_INHIBIT on the next command, but the ladder verifies immediately, so wait here.
        if !wait_clear(base, STATUS, ST_DAT_INHIBIT, DATA_TIMEOUT_MS) {
            serial_println!("{}   ladder: card stayed busy (DAT0) after writing LBA {} — programming did not complete ::", PS, lba);
            return false;
        }
        true
    }

    /// Emergency restore after a mid-ladder fault (a write may have partially landed): re-write the stash
    /// and verify it. If the restore itself cannot be verified, dump the stashed original as hex so the
    /// data is never silently lost. Returns whether the original was provably put back.
    #[cfg(feature = "sdmmc_arm")]
    fn restore_or_dump(base: u64, card: &Card, lba: u64, stash: &[u8; 512]) -> bool {
        serial_println!("{}   ladder: emergency RESTORE of LBA {} after a mid-ladder fault ::", PS, lba);
        if write_block_at(base, card, lba, stash) {
            let mut chk = [0u8; 512];
            if read_block_at(base, card, lba, &mut chk) && chk == *stash {
                serial_println!("{}   ladder: emergency restore verified — original data preserved ::", PS);
                return true;
            }
        }
        serial_println!("{}   ladder: EMERGENCY RESTORE FAILED — original {}-byte stash below as hex so it is never lost ::", PS, 512);
        dump_hex(stash);
        false
    }

    /// The paranoia ladder (ORIN-SDMMC-2). Announced-before-issue, bounded, and RESTORE-by-construction:
    ///  1. re-run the rung-1 read census (sector 0) and confirm it is stable;
    ///  2. pick the SCRATCH REGION — the last `SCRATCH_BLOCKS` block(s) of the card, ONLY IF sector 0 shows
    ///     no GPT (a GPT backup header lives in the last LBA — with GPT present we REFUSE scratch writes this
    ///     arc and say so);
    ///  3. read + stash the scratch region's current contents;
    ///  4. single-block CMD24 write of a stamped pattern;
    ///  5. read-back + byte-compare (the write verified);
    ///  6. RESTORE the stashed original contents;
    ///  7. read-back + byte-compare the restoration.
    /// Emits `:: SDMMC: write ladder — write/verify/restore/verify => PASS ::` only if EVERY step verified.
    /// Any mismatch = a distinct FAIL line naming the step; if a write landed, an emergency restore runs,
    /// and if the restore itself cannot be verified the stashed original is dumped as hex.
    #[cfg(feature = "sdmmc_arm")]
    fn write_ladder(base: u64, card: &Card, sec0: &[u8; 512]) {
        serial_println!(
            "{} ORIN-SDMMC-2 ARMED (UNAOS_SDMMC_ARM=1) — paranoia write ladder on the SEATED card (scratch region, stashed + restored) ::",
            PS
        );

        // ── Step 1/7: re-run the rung-1 read census and confirm it is stable ──
        serial_println!("{}   ladder step 1/7: re-reading sector 0 (rung-1 read census) before any write ::", PS);
        let mut recensus = [0u8; 512];
        if !read_block_at(base, card, 0, &mut recensus) {
            serial_println!("{}   ladder FAIL step 1 (re-census): sector-0 re-read failed — REFUSING to proceed ::", PS);
            return;
        }
        if recensus != *sec0 {
            serial_println!("{}   ladder FAIL step 1 (re-census): sector 0 changed since the census read — REFUSING to proceed ::", PS);
            return;
        }
        serial_println!("{}   ladder step 1: read census stable (sector 0 re-read byte-identical) ::", PS);

        // ── Step 2/7: pick the scratch region (the GPT-refusal rule) ──
        let class = classify_sector0(sec0);
        if class.starts_with("GPT") {
            serial_println!(
                "{}   ladder step 2/7: sector 0 is {} — a GPT BACKUP header lives in the card's LAST LBA, exactly where our scratch region sits; REFUSING all scratch writes this arc (no provably-safe region) ::",
                PS, class
            );
            serial_println!("{} ORIN-SDMMC-2 write ladder REFUSED (GPT present — the seated card is sacred; no write) ::", PS);
            return;
        }
        if card.num_blocks < SCRATCH_BLOCKS {
            serial_println!("{}   ladder step 2/7: card too small ({} blocks) for a {}-block scratch region — REFUSING ::", PS, card.num_blocks, SCRATCH_BLOCKS);
            return;
        }
        let scratch_lba = card.num_blocks - SCRATCH_BLOCKS;
        serial_println!(
            "{}   ladder step 2/7: no GPT (sector 0 = {}) — scratch region = the last {} block(s), LBA {} (card's last LBA) ::",
            PS, class, SCRATCH_BLOCKS, scratch_lba
        );

        // ── Step 3/7: read + stash the scratch region's current contents ──
        serial_println!("{}   ladder step 3/7: reading + stashing scratch LBA {} current contents ::", PS, scratch_lba);
        let mut stash = [0u8; 512];
        if !read_block_at(base, card, scratch_lba, &mut stash) {
            serial_println!("{}   ladder FAIL step 3 (stash read): could not read scratch LBA {} — REFUSING to write (nothing to restore from) ::", PS, scratch_lba);
            return;
        }
        serial_println!(
            "{}   ladder step 3: stashed 512 bytes from LBA {} (first 8: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}) ::",
            PS, scratch_lba,
            stash[0], stash[1], stash[2], stash[3], stash[4], stash[5], stash[6], stash[7]
        );

        // ── Step 4/7: single-block CMD24 write of a stamped pattern ──
        let mut pattern = [0u8; 512];
        make_pattern(&mut pattern, scratch_lba);
        serial_println!("{}   ladder step 4/7: CMD24 single-block WRITE of stamped pattern to LBA {} ::", PS, scratch_lba);
        if !write_block_at(base, card, scratch_lba, &pattern) {
            serial_println!("{}   ladder FAIL step 4 (write): CMD24 write to LBA {} failed ::", PS, scratch_lba);
            // The write may have partially landed — attempt to put the original back.
            restore_or_dump(base, card, scratch_lba, &stash);
            return;
        }

        // ── Step 5/7: read-back + byte-compare against the stamped pattern ──
        serial_println!("{}   ladder step 5/7: reading back LBA {} + byte-comparing to the stamped pattern ::", PS, scratch_lba);
        let mut readback = [0u8; 512];
        if !read_block_at(base, card, scratch_lba, &mut readback) {
            serial_println!("{}   ladder FAIL step 5 (verify read): read-back of LBA {} failed ::", PS, scratch_lba);
            restore_or_dump(base, card, scratch_lba, &stash);
            return;
        }
        if readback != pattern {
            serial_println!("{}   ladder FAIL step 5 (verify): read-back != written pattern at LBA {} ::", PS, scratch_lba);
            restore_or_dump(base, card, scratch_lba, &stash);
            return;
        }
        serial_println!("{}   ladder step 5: write verified (read-back byte-identical to the stamped pattern) ::", PS);

        // ── Step 6/7: RESTORE the stashed original contents ──
        serial_println!("{}   ladder step 6/7: RESTORING original stashed contents to LBA {} ::", PS, scratch_lba);
        if !write_block_at(base, card, scratch_lba, &stash) {
            serial_println!("{}   ladder FAIL step 6 (restore write): CMD24 restore to LBA {} FAILED — original {}-byte data below as hex so it is never lost ::", PS, scratch_lba, 512);
            dump_hex(&stash);
            return;
        }

        // ── Step 7/7: read-back + byte-compare the restoration ──
        serial_println!("{}   ladder step 7/7: reading back LBA {} + byte-comparing to the stash (restore verify) ::", PS, scratch_lba);
        let mut restored = [0u8; 512];
        if !read_block_at(base, card, scratch_lba, &mut restored) {
            serial_println!("{}   ladder FAIL step 7 (restore verify read): read-back of LBA {} failed — original data below as hex ::", PS, scratch_lba);
            dump_hex(&stash);
            return;
        }
        if restored != stash {
            serial_println!("{}   ladder FAIL step 7 (restore verify): restored contents != stash at LBA {} — original data below as hex ::", PS, scratch_lba);
            dump_hex(&stash);
            return;
        }
        serial_println!("{}   ladder step 7: restore verified (LBA {} byte-identical to the original stash) ::", PS, scratch_lba);

        // Every step verified — the scratch region is provably back to its original contents.
        serial_println!("{} write ladder — write/verify/restore/verify => PASS ::", PS);
    }

    /// Build the stamped scratch pattern: a recognisable ASCII marker + the target LBA + a byte-position
    /// sweep, so a stuck-bit, a wrong-LBA landing, or a partial write all fail the byte-compare loudly.
    #[cfg(feature = "sdmmc_arm")]
    fn make_pattern(buf: &mut [u8; 512], lba: u64) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8) ^ 0x5a;
        }
        const MARK: &[u8] = b"UNAOS-SDMMC2-SCRATCH";
        buf[..MARK.len()].copy_from_slice(MARK);
        buf[32..40].copy_from_slice(&lba.to_le_bytes());
    }

    // ══════════════════════════════════════════════════════════════════════════════════════════════════
    // ORIN-INSTALL-2 — the self-clone: the installer copies the RUNNING system's real boot payload. Drive
    // the arch-neutral installer ENGINE (`crate::install`: GPT writer + FAT32 formatter + the INSTALL-2
    // `TreeWriter` + sha extent-verify) onto the SEATED microSD via the rung-2 armed single-block write
    // primitives, with the payload READ FROM the USB boot stick's own ESP (the `fs::fat` mount/read path).
    // `install_target`-gated (implies `sdmmc_arm` ⇒ `sdmmc`): EVERY line below this marker is compiled out
    // unless UNAOS_INSTALL_TARGET_SD=1, so a plain `sdmmc`/`sdmmc_arm` build is byte-identical to the recon.
    //
    // THE THREE-GATE ESCALATION LADDER (the seated card is sacred; a real install is the most destructive
    // act in the tree, so it stands behind three independent gates):
    //   Gate 1 — `sdmmc`         : the controller is up and the card CENSUS succeeded (we hold a `Card`).
    //   Gate 2 — `sdmmc_arm`     : the rung-2 armed write path (CMD24) is compiled in.
    //   Gate 3 — `install_target`: THIS flag, the explicit DESTRUCTIVE-CONFIRMATION gate. On metal the
    //                              future UX asks the operator; this arc the knob stands in for that, and
    //                              the flow prints exactly what it is about to destroy (sector-0
    //                              classification + the card's CID identity) BEFORE the first write.
    // Unlike the engine's blank-only demo law, an installer must handle a NON-blank card: we do not refuse
    // it — we announce it, then re-establish the FAT blank-precondition by zeroing exactly the ESP metadata
    // region (`fat32::blank_region_sectors`) before formatting. The engine's verified write/verify
    // semantics are untouched — this module supplies the block target, the zero pass, the announce, and the
    // USB→SD tree copy; every copied file is sha-extent-verified through the engine's own `verify_extents`.
    //
    // THE INSTALL-1 BLOCKER, RESOLVED (the position adjudication): INSTALL-1 ran at the pre-JB2b census
    // site where `drivers::block::info()` is None (the stick is not yet enumerated), so it fell back to a
    // synthetic marker. INSTALL-2 DEFERS the destructive install to `sdmmc_install_from_usb`, called from
    // the boot sequence AFTER the JB2b pump enumerated the stick — the earliest position where the payload
    // is readable, the SDMMC MMIO is still mapped, and the core is still at EL2 (pre-JM6 drop, timer live
    // for the SD bounded waits). The census stashes the read-only identity into `PENDING_INSTALL`.
    // ══════════════════════════════════════════════════════════════════════════════════════════════════

    /// The card identity the census stashes for the deferred post-JB2b install (see the block header). A
    /// `None` at the deferred site means the census never identified a card (no controller / no card), so
    /// the install site prints an honest skip and does nothing.
    #[cfg(feature = "install_target")]
    static PENDING_INSTALL: spin::Mutex<Option<(u64, Card, [u8; 512])>> = spin::Mutex::new(None);

    // ── ORIN-SDMMC-3: multi-block transfer primitives (CMD18 READ_MULTIPLE / CMD25 WRITE_MULTIPLE) ──
    //
    // The rung-1/rung-2 ladder moves one 512-byte block per command (CMD17/CMD24); a multi-MB kernel.elf
    // is thousands of single-block commands. These primitives collapse a run of contiguous blocks into a
    // single command with a block count, per the SDHCI multi-block model:
    //   * BLKSIZECNT carries the block COUNT in [31:16] (with block size 512 in [15:0]);
    //   * the Transfer-Mode field (CMDTM[15:0]) sets Block-Count-Enable + Multi-Block-Select;
    //   * COMPLETION uses **auto-CMD12**: the host controller issues STOP_TRANSMISSION itself at the end
    //     of the counted transfer. We choose auto-CMD12 over an explicit CMD12 so there is no second
    //     command round-trip and no separate CMD12 error-handling path — the controller closes the
    //     open-ended read/write for us, and normal transfer-complete (INT_DATA_DONE) still fires.
    // Both are `install_target`-gated (⇒ sdmmc_arm ⇒ sdmmc), so a plain `sdmmc`/`sdmmc_arm` build carries
    // NEITHER and stays byte-for-byte identical to the merged recon/ladder. The multi-block WRITE therefore
    // exists only behind the armed (sdmmc_arm) gate as the arc requires; the rung-2 witness ladder is left
    // entirely on single-block CMD24 (its metal-verified semantics unchanged).

    /// Transfer-Mode bits inside CMDTM[15:0] (standard SDHCI; distinct from the command bits [16:31]).
    #[cfg(feature = "install_target")]
    const TM_BLKCNT_EN: u32 = 1 << 1; // Block Count Enable — BLKSIZECNT[31:16] is a valid count
    #[cfg(feature = "install_target")]
    const TM_AUTO_CMD12: u32 = 1 << 2; // Auto CMD12 Enable — controller issues STOP at transfer end
    #[cfg(feature = "install_target")]
    const TM_MULTI_BLK: u32 = 1 << 5; // Multi/Single Block Select — 1 = multi-block

    /// Bounded multi-block chunk: the most 512-byte blocks a single CMD18/CMD25 transfer moves. 64 blocks
    /// (32 KiB) keeps one transfer's PIO drain + bounded-wait budget modest while collapsing a run into
    /// ~1/64 the command count of the single-block path. `SdInstallTarget` loops this bound (and drops to
    /// the single-block CMD17/CMD24 primitive for a 1-block tail — the retained fallback).
    #[cfg(feature = "install_target")]
    const MULTIBLOCK_CHUNK_BLOCKS: u32 = 64;

    /// Read `count` contiguous blocks starting at `lba` via a polled CMD18 READ_MULTIPLE_BLOCK with
    /// block-count + auto-CMD12. `buf` must be exactly `count * 512` bytes. READ-ONLY. Returns whether the
    /// whole run was read. Generalises `read_block_at` to a counted transfer.
    #[cfg(feature = "install_target")]
    fn read_blocks_at(base: u64, card: &Card, lba: u64, buf: &mut [u8], count: u32) -> bool {
        debug_assert!(buf.len() == count as usize * 512);
        let arg = if card.block_addressing { lba as u32 } else { (lba * 512) as u32 };
        write32(base, INTERRUPT, 0xffff_ffff);
        write32(base, BLKSIZECNT, (count << 16) | 512);
        if send_command(
            base,
            cmd(18)
                | CMD_RESP_48
                | CMD_CRCCHK
                | CMD_IXCHK
                | CMD_ISDATA
                | CMD_DAT_DIR_READ
                | TM_BLKCNT_EN
                | TM_MULTI_BLK
                | TM_AUTO_CMD12,
            arg,
        )
        .is_err()
        {
            serial_println!("{}   mb: CMD18 (READ {} blk @LBA {}) failed at the link layer ::", PS, count, lba);
            return false;
        }
        let r1 = read32(base, RESP0);
        if r1 & R1_ERROR_MASK != 0 {
            serial_println!("{}   mb: CMD18 (LBA {}) R1 error status {:#010x} ::", PS, lba, r1);
            return false;
        }
        for blk in 0..count as usize {
            if !wait_set(base, INTERRUPT, INT_READ_RDY, DATA_TIMEOUT_MS) {
                serial_println!("{}   mb: read buffer never ready (block {} of {} @LBA {}) ::", PS, blk, count, lba);
                return false;
            }
            write32(base, INTERRUPT, INT_READ_RDY); // W1C, re-arm for the next block
            let bo = blk * 512;
            for i in 0..128usize {
                let word = read32(base, DATA);
                let off = bo + i * 4;
                buf[off..off + 4].copy_from_slice(&word.to_le_bytes());
            }
        }
        // Transfer-complete: the controller's auto-CMD12 has closed the read.
        if !wait_set(base, INTERRUPT, INT_DATA_DONE | INT_ERR_ANY, DATA_TIMEOUT_MS) {
            serial_println!("{}   mb: transfer-complete timeout after {} blocks @LBA {} ::", PS, count, lba);
            return false;
        }
        let int = read32(base, INTERRUPT);
        write32(base, INTERRUPT, int);
        if int & INT_ERR_ANY != 0 {
            serial_println!("{}   mb: data error {:#010x} reading {} blocks @LBA {} ::", PS, int, count, lba);
            return false;
        }
        true
    }

    /// Write `count` contiguous blocks starting at `lba` via a polled CMD25 WRITE_MULTIPLE_BLOCK with
    /// block-count + auto-CMD12. `buf` must be exactly `count * 512` bytes. The multi-block card-write —
    /// reachable only under `install_target` (⇒ `sdmmc_arm`), only from `SdInstallTarget`. Returns whether
    /// the whole run was written AND the card left the programming (busy) state cleanly.
    #[cfg(feature = "install_target")]
    fn write_blocks_at(base: u64, card: &Card, lba: u64, buf: &[u8], count: u32) -> bool {
        debug_assert!(buf.len() == count as usize * 512);
        let arg = if card.block_addressing { lba as u32 } else { (lba * 512) as u32 };
        write32(base, INTERRUPT, 0xffff_ffff);
        write32(base, BLKSIZECNT, (count << 16) | 512);
        // CMD25 WRITE_MULTIPLE_BLOCK: host->card (DAT_DIR_READ clear) + block-count + multi-block + auto-CMD12.
        if send_command(
            base,
            cmd(25)
                | CMD_RESP_48
                | CMD_CRCCHK
                | CMD_IXCHK
                | CMD_ISDATA
                | TM_BLKCNT_EN
                | TM_MULTI_BLK
                | TM_AUTO_CMD12,
            arg,
        )
        .is_err()
        {
            serial_println!("{}   mb: CMD25 (WRITE {} blk @LBA {}) failed at the link layer ::", PS, count, lba);
            return false;
        }
        let r1 = read32(base, RESP0);
        if r1 & R1_ERROR_MASK != 0 {
            serial_println!("{}   mb: CMD25 (LBA {}) R1 error status {:#010x} ::", PS, lba, r1);
            return false;
        }
        for blk in 0..count as usize {
            if !wait_set(base, INTERRUPT, INT_WRITE_RDY, DATA_TIMEOUT_MS) {
                serial_println!("{}   mb: write buffer never ready (block {} of {} @LBA {}) ::", PS, blk, count, lba);
                return false;
            }
            write32(base, INTERRUPT, INT_WRITE_RDY); // W1C, re-arm for the next block
            let bo = blk * 512;
            for i in 0..128usize {
                let off = bo + i * 4;
                let word = u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
                write32(base, DATA, word);
            }
        }
        // Transfer-complete: the controller's auto-CMD12 has closed the write.
        if !wait_set(base, INTERRUPT, INT_DATA_DONE | INT_ERR_ANY, DATA_TIMEOUT_MS) {
            serial_println!("{}   mb: transfer-complete timeout after {} blocks @LBA {} ::", PS, count, lba);
            return false;
        }
        let int = read32(base, INTERRUPT);
        write32(base, INTERRUPT, int);
        if int & INT_ERR_ANY != 0 {
            serial_println!("{}   mb: data error {:#010x} writing {} blocks @LBA {} ::", PS, int, count, lba);
            return false;
        }
        // The card asserts DAT0 busy while it programs the run's flash; wait for release before treating
        // the write as durable (a following read would otherwise race the internal programming).
        if !wait_clear(base, STATUS, ST_DAT_INHIBIT, DATA_TIMEOUT_MS) {
            serial_println!("{}   mb: card stayed busy (DAT0) after writing {} blocks @LBA {} — programming did not complete ::", PS, count, lba);
            return false;
        }
        true
    }

    /// An `InstallTarget` over the SD write path. Sector granularity; `read_sectors`/`write_sectors` move
    /// the bulk of a multi-sector buffer with the ORIN-SDMMC-3 multi-block CMD18/CMD25 primitives (bounded
    /// to `MULTIBLOCK_CHUNK_BLOCKS` per transfer) and drop to the rung-2 single-block CMD17/CMD24 primitives
    /// for a 1-block tail (retained fallback). A 512-byte call (the FAT-metadata path) is therefore always
    /// a single-block command; a whole-file write (the batched `TreeWriter::write_file`) rides multi-block.
    #[cfg(feature = "install_target")]
    struct SdInstallTarget<'a> {
        base: u64,
        card: &'a Card,
    }

    #[cfg(feature = "install_target")]
    impl crate::install::InstallTarget for SdInstallTarget<'_> {
        fn capacity_sectors(&self) -> u64 {
            self.card.num_blocks
        }

        fn id(&self) -> alloc::string::String {
            let mid = r2_bits(&self.card.cid, 127, 120) as u8;
            let psn = r2_bits(&self.card.cid, 55, 24) as u32;
            alloc::format!(
                "Orin microSD (MID {:#04x}, serial {:#010x}, {} x 512B sectors)",
                mid, psn, self.card.num_blocks
            )
        }

        fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), crate::install::InstallError> {
            debug_assert!(buf.len() % 512 == 0);
            let total = (buf.len() / 512) as u64;
            let mut done: u64 = 0;
            while done < total {
                let n = core::cmp::min(MULTIBLOCK_CHUNK_BLOCKS as u64, total - done) as u32;
                let region = &mut buf[done as usize * 512..(done as usize + n as usize) * 512];
                let ok = if n == 1 {
                    // Single-block tail (and the 512-byte metadata path): the retained CMD17 fallback.
                    let mut sec = [0u8; 512];
                    let r = read_block_at(self.base, self.card, lba + done, &mut sec);
                    if r {
                        region.copy_from_slice(&sec);
                    }
                    r
                } else {
                    read_blocks_at(self.base, self.card, lba + done, region, n)
                };
                if !ok {
                    return Err(crate::install::InstallError::Io);
                }
                done += n as u64;
            }
            Ok(())
        }

        fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), crate::install::InstallError> {
            debug_assert!(buf.len() % 512 == 0);
            let total = (buf.len() / 512) as u64;
            let mut done: u64 = 0;
            while done < total {
                let n = core::cmp::min(MULTIBLOCK_CHUNK_BLOCKS as u64, total - done) as u32;
                let region = &buf[done as usize * 512..(done as usize + n as usize) * 512];
                let ok = if n == 1 {
                    // Single-block tail (and the 512-byte metadata path): the retained CMD24 fallback.
                    let mut sec = [0u8; 512];
                    sec.copy_from_slice(region);
                    write_block_at(self.base, self.card, lba + done, &sec)
                } else {
                    write_blocks_at(self.base, self.card, lba + done, region, n)
                };
                if !ok {
                    return Err(crate::install::InstallError::Io);
                }
                done += n as u64;
            }
            Ok(())
        }
    }

    /// The DEFERRED install entry point (ORIN-INSTALL-2). Called from the boot sequence AFTER the JB2b
    /// pump window has enumerated the USB boot stick as a block device — the position where the running
    /// system's real boot payload is readable (`drivers::block::info()` is Some). Consumes the card
    /// identity the census stashed. Honest no-op if the census never identified a card, or if the stick
    /// did not enumerate (no `drivers::block` device) — in either case the flow needs a self to clone, so
    /// it prints the honest cause and does nothing destructive. On metal (tegra) only; QEMU virt has no
    /// Tegra234 SDMMC and never stashes.
    #[cfg(feature = "install_target")]
    pub fn sdmmc_install_from_usb() {
        let ctx = PENDING_INSTALL.lock().take();
        let (base, card, sec0) = match ctx {
            Some(c) => c,
            None => {
                serial_println!(
                    "{} ORIN-INSTALL-2 deferred install SKIPPED — no card identity stashed (census found no controller/card) ::",
                    PS
                );
                return;
            }
        };
        // The self must be readable: the USB boot stick must have enumerated as a block device in the
        // JB2b pump window. If it did not, there is no running boot payload to clone — honest skip.
        if crate::drivers::block::info().is_none() {
            serial_println!(
                "{} ORIN-INSTALL-2 deferred install SKIPPED — the USB boot stick did not enumerate as a block device (no self to clone) ::",
                PS
            );
            return;
        }
        install_to_sd(base, &card, &sec0);
    }

    /// ORIN-INSTALL-2: run the installer engine end-to-end against the seated microSD, cloning the USB
    /// boot stick's real ESP payload. The flow: announce → GPT → zero ESP metadata (blank-precondition) →
    /// FAT32 format → mount the stick + copy its boot tree file-by-file (each sha-extent-verified) →
    /// per-file sha manifest → the INSTALL PASS line. Any engine/read error is a named FAIL line (no panic).
    #[cfg(feature = "install_target")]
    fn install_to_sd(base: u64, card: &Card, sec0: &[u8; 512]) {
        serial_println!(
            "{} ORIN-INSTALL-2 THIRD GATE (UNAOS_INSTALL_TARGET_SD=1) — cloning the running boot payload onto the SEATED microSD ::",
            PS
        );
        serial_println!(
            "{}   gates: [1] sdmmc census OK · [2] sdmmc_arm write path armed · [3] install_target destructive-confirm — all satisfied ::",
            PS
        );

        // ── About-to-destroy announcement: classify sector 0 + re-print the card CID identity. ──
        let class = classify_sector0(sec0);
        serial_println!(
            "{}   ABOUT TO DESTROY: microSD sector-0 = {} · capacity {} blocks ({} MiB) — the entire card is about to be repartitioned ::",
            PS, class, card.num_blocks, card.num_blocks * 512 / (1024 * 1024)
        );
        print_cid(&card.cid);

        let mut t = SdInstallTarget { base, card };
        match install_flow(&mut t) {
            Ok(n) => serial_println!(
                "{} ORIN-INSTALL-2 SD install — gpt+zero+fat32+clone({} files) verify => PASS ::",
                PS, n
            ),
            Err(e) => serial_println!("{} ORIN-INSTALL-2 SD install => FAIL ({:?}) ::", PS, e),
        }
    }

    /// A copied file's verification record: its path on the boot tree, content SHA-256, and the exact
    /// device extents the writer recorded (what `verify_extents` re-reads off the card).
    #[cfg(feature = "install_target")]
    struct FileRec {
        path: alloc::string::String,
        sha: [u8; 32],
        extents: alloc::vec::Vec<crate::install::fat32::Extent>,
        size: usize,
    }

    /// The engine driver: returns the number of files cloned (so `?` early-returns land on the single
    /// FAIL line above). Mounts the USB boot stick and mirrors its ESP tree onto the freshly-formatted
    /// microSD ESP, sha-extent-verifying every copied file.
    #[cfg(feature = "install_target")]
    fn install_flow(t: &mut SdInstallTarget) -> Result<usize, crate::install::InstallError> {
        use crate::install::{fat32, gpt, verify_extents, InstallError, InstallTarget};

        // 1) GPT: protective MBR + primary/backup + ESP + data, with the engine's own parse-back verify.
        let layout = gpt::write_gpt(t)?;
        serial_println!(
            "{}   INSTALL: GPT written + parse-back verified — ESP LBA {}..{}, data LBA {}..{} of {} sectors ::",
            PS, layout.esp_first_lba, layout.esp_last_lba, layout.data_first_lba, layout.data_last_lba,
            layout.total_sectors
        );

        // 2) Re-establish the FAT blank-precondition: zero exactly the ESP reserved+FAT region (the card
        //    may be non-blank; a stale FAT entry would forge an allocation). Data-area directory clusters
        //    are built wholly in memory by the TreeWriter, so no data-area pre-zero is needed.
        let esp_sectors = layout.esp_last_lba - layout.esp_first_lba + 1;
        let blank_sectors = fat32::blank_region_sectors(esp_sectors)?;
        let zero = [0u8; 512];
        for s in 0..blank_sectors {
            t.write_sectors(layout.esp_first_lba + s, &zero)?;
        }
        serial_println!(
            "{}   INSTALL: zeroed {} ESP metadata sectors (reserved + both FATs) to re-establish the blank-precondition ::",
            PS, blank_sectors
        );

        // 3) FAT32 format the ESP.
        let geom = fat32::format_esp(t, layout.esp_first_lba, esp_sectors)?;
        serial_println!(
            "{}   INSTALL: ESP formatted FAT32 — fat_sz={}sec clusters={} data@vol+{} ::",
            PS, geom.fat_sz, geom.count_of_clusters, geom.data_start
        );

        // 4) THE SELF-CLONE: mount the USB boot stick's own ESP through the in-tree FAT reader and mirror
        //    its whole boot tree onto the microSD ESP. `fs::fat::mount()` reads `drivers::block` — the USB
        //    path — which the JB2b pump populated before this deferred site ran.
        let src = crate::fs::fat::mount().map_err(|_| InstallError::NotReady)?;
        serial_println!("{}   INSTALL: mounted USB boot stick — {} ::", PS, src.describe());

        let mut recs: alloc::vec::Vec<FileRec> = alloc::vec::Vec::new();
        {
            let mut w = fat32::TreeWriter::new(t, geom);
            let root_entries = src.read_root().map_err(|_| InstallError::Io)?;
            // Size the root directory to its entry count (multi-cluster if >16 entries) and reserve its
            // cluster chain BEFORE any file/subdir allocation (so cluster 2's chain stays contiguous).
            let root_slots = count_nondot(&root_entries);
            let root_clusters = fat32::dir_clusters_for_slots(root_slots);
            let root_cluster = w.reserve_root(root_clusters)?;
            copy_dir(&mut w, &src, &root_entries, root_cluster, 0, true, "", &mut recs, 0, root_clusters)?;
            serial_println!(
                "{}   INSTALL: cloned {} files ({} data clusters) from the boot tree ::",
                PS, recs.len(), w.clusters_used()
            );
        }
        if recs.is_empty() {
            // A boot stick with no files is not a self to clone — refuse rather than "PASS" a hollow card.
            serial_println!("{}   INSTALL: the USB boot stick carried no files to clone => FAIL ::", PS);
            return Err(InstallError::BadArg);
        }

        // 5) Content-verify EVERY copied file by re-reading its extents off the card and SHA-checking — the
        //    installer's content-verify IS the bench's content-verify, now native. Print the per-file sha
        //    manifest (the real manifest that replaces INSTALL-1's single UNAOS.IMG marker).
        for r in &recs {
            if !verify_extents(t, &r.extents, &r.sha)? {
                serial_println!("{}   INSTALL: {} extent sha-verify => FAIL ::", PS, r.path);
                return Err(InstallError::VerifyFailed);
            }
            serial_println!(
                "{}   INSTALL: {} ({} B, {} extents) sha256={} => VERIFIED ::",
                PS, r.path, r.size, r.extents.len(), sha_hex(&r.sha)
            );
        }
        serial_println!(
            "{}   INSTALL: all {} cloned files re-read off the card + sha-verified => PASS ::",
            PS, recs.len()
        );
        // NOTE: the in-tree `fs::fat::mount()` interop self-check the x86 engine witness runs on ITS target
        // is not run on the SD here — `mount()` reads `drivers::block` (the USB source), not this armed SD
        // target. The per-file SD content-verify above IS the by-content proof.
        Ok(recs.len())
    }

    /// Depth-first mirror of a source directory onto the SD `TreeWriter`. Builds THIS directory's cluster
    /// wholly in memory (so a stale data cluster never leaks bytes), recursing into subdirectories before
    /// writing their parent entry (children must exist to know their first cluster). Skips `.`/`..`; every
    /// file is read whole off the stick, its clusters allocated + written, and its record pushed for the
    /// caller's sha-extent verify. `is_root` selects whether `.`/`..` are emitted and whether a child's
    /// `..` points at cluster 0 (the FAT convention for a subdirectory of the root).
    /// Count the entries in a source directory that are neither `.` nor `..` (the slots a mirrored copy of
    /// it must hold beyond its own `.`/`..`). Used to size a directory's cluster chain up front.
    #[cfg(feature = "install_target")]
    fn count_nondot(entries: &[crate::fs::fat::DirEntry]) -> usize {
        entries
            .iter()
            .filter(|e| {
                let n = e.name();
                n != "." && n != ".."
            })
            .count()
    }

    #[cfg(feature = "install_target")]
    #[allow(clippy::too_many_arguments)]
    fn copy_dir(
        w: &mut crate::install::fat32::TreeWriter<'_, SdInstallTarget<'_>>,
        src: &crate::fs::fat::FatFs,
        entries: &[crate::fs::fat::DirEntry],
        this_cluster: u32,
        parent_cluster: u32,
        is_root: bool,
        path_prefix: &str,
        recs: &mut alloc::vec::Vec<FileRec>,
        depth: u32,
        this_clusters: u32,
    ) -> Result<(), crate::install::InstallError> {
        use crate::install::fat32::{
            dir_clusters_for_slots, put_dir_entry, ATTR_ARCHIVE, ATTR_DIR, DIR_SLOTS_PER_CLUSTER,
        };
        use crate::install::InstallError;

        // A sane recursion bound (the boot ESP tree is 2 levels: root → EFI → BOOT); refuse a pathological
        // or looping source tree rather than blow the kernel stack.
        const MAX_DEPTH: u32 = 8;
        if depth > MAX_DEPTH {
            return Err(InstallError::BadArg);
        }
        // Per-file size cap — the boot payload is a few MB; refuse an implausibly large file rather than
        // exhaust the 48 MiB kernel heap reading it whole.
        const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;

        // The directory image is built WHOLLY in memory (no stale-byte leak) across its whole cluster
        // chain, then written once. `this_clusters` was sized by the caller from this dir's entry count, so
        // a directory with >16 entries spans >1 cluster — the lifted single-cluster bound.
        let capacity = this_clusters as usize * DIR_SLOTS_PER_CLUSTER;
        let mut dir = alloc::vec![0u8; this_clusters as usize * 512];
        let mut slot = 0usize;
        if !is_root {
            if !put_dir_entry(&mut dir, slot, ".", ATTR_DIR, this_cluster, 0) {
                return Err(InstallError::BadArg);
            }
            slot += 1;
            if !put_dir_entry(&mut dir, slot, "..", ATTR_DIR, parent_cluster, 0) {
                return Err(InstallError::BadArg);
            }
            slot += 1;
        }
        for e in entries {
            let name = e.name();
            if name == "." || name == ".." {
                continue;
            }
            if slot >= capacity {
                // Sized from the entry count up front, so this is a genuine overflow (e.g. a source dir
                // that changed under us) — honest NoSpace, never silent truncation.
                return Err(InstallError::NoSpace);
            }
            if e.is_dir {
                let child_entries = src.read_dir(e.first_cluster()).map_err(|_| InstallError::Io)?;
                // Size the child (its own `.`/`..` + its non-dot entries) and allocate its chain.
                let child_clusters = dir_clusters_for_slots(2 + count_nondot(&child_entries));
                let child = w.alloc_dir_clusters(child_clusters)?;
                let child_prefix = alloc::format!("{}{}/", path_prefix, name);
                let child_dotdot = if is_root { 0 } else { this_cluster };
                copy_dir(w, src, &child_entries, child, child_dotdot, false, &child_prefix, recs, depth + 1, child_clusters)?;
                if !put_dir_entry(&mut dir, slot, name, ATTR_DIR, child, 0) {
                    return Err(InstallError::BadArg);
                }
                slot += 1;
            } else {
                let size = e.size as usize;
                if size > MAX_FILE_BYTES {
                    serial_println!(
                        "{}   INSTALL: {}{} is {} B (> {} B cap) — refusing => FAIL ::",
                        PS, path_prefix, name, size, MAX_FILE_BYTES
                    );
                    return Err(InstallError::BadArg);
                }
                let mut data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                src.read_file(e, &mut data, size).map_err(|_| InstallError::Io)?;
                if data.len() != size {
                    // Short read (chain ended before de.size) — the source is malformed; do not clone it.
                    return Err(InstallError::Io);
                }
                let sha = crate::install::hash::sha256(&data);
                let (first, extents) = w.write_file(&data)?;
                if !put_dir_entry(&mut dir, slot, name, ATTR_ARCHIVE, first, size as u32) {
                    return Err(InstallError::BadArg);
                }
                slot += 1;
                recs.push(FileRec {
                    path: alloc::format!("{}{}", path_prefix, name),
                    sha,
                    extents,
                    size,
                });
            }
        }
        w.write_dir_image(this_cluster, &dir)?;
        Ok(())
    }

    /// Lower-hex a 32-byte digest for the per-file manifest line.
    #[cfg(feature = "install_target")]
    fn sha_hex(d: &[u8; 32]) -> alloc::string::String {
        let mut s = alloc::string::String::with_capacity(64);
        for b in d {
            s.push(core::char::from_digit((b >> 4) as u32, 16).unwrap());
            s.push(core::char::from_digit((b & 0xf) as u32, 16).unwrap());
        }
        s
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

        // ── ORIN-SDMMC-2: the paranoia write ladder (only with UNAOS_SDMMC_ARM=1; compiled out otherwise, so
        //    a plain UNAOS_SDMMC=1 build ends exactly here, byte-identical to the merged recon). ──
        #[cfg(feature = "sdmmc_arm")]
        write_ladder(base, &card, &sec0);

        // ── ORIN-INSTALL-2: the self-clone installer flow (only with UNAOS_INSTALL_TARGET_SD=1, the third
        //    destructive gate; compiled out otherwise, so a plain UNAOS_SDMMC_ARM=1 build ends at the ladder
        //    above, byte-identical to the merged rung-2). INSTALL-1 ran the install HERE and had to fall
        //    back to a synthetic marker payload — at this pre-JB2b-takeover site the USB boot stick is not
        //    yet a block device, so the running system's real boot payload is unreadable. INSTALL-2 splits
        //    the act in two: the census STASHES the read-only card identity here, and the destructive
        //    install is DEFERRED to `sdmmc_install_from_usb` — called from the boot sequence AFTER the JB2b
        //    pump window has enumerated the stick as a block device, where the real payload IS readable.
        //    See arch_arm64.md §ORIN-INSTALL-2. ──
        #[cfg(feature = "install_target")]
        {
            *PENDING_INSTALL.lock() = Some((base, card, sec0));
            serial_println!(
                "{} ORIN-INSTALL-2 card identity stashed; destructive install DEFERRED to the post-JB2b USB-enumerated site (self-clone needs the boot stick readable) ::",
                PS
            );
        }
    }
}
