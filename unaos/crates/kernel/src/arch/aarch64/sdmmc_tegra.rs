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
//     rails up (the bootloader read the card to boot). We do NOT program the CAR/BPMP clock, and we
//     AUTHOR no Tegra vendor pad-control value (>= 0x100) — we drive ONLY the standard SDHCI
//     internal-clock divider (CONTROL1) off whatever base clock the controller already has running. If
//     metal shows the internal clock never stabilises (CLK_STABLE never sets), the diagnosis is "the
//     input clock is gated" and the fix (a BPMP clock MRQ) is a later arc — surfaced, never worked
//     around.
//     M2b AMENDMENT (post boot 3): "author none" is not the same as "never write". SRST_ALL returns the
//     vendor block to power-on defaults, which DISCARDS the tap/trim/drive-strength/auto-cal the
//     bootloader calibrated for this board — the ladder then runs on pads the firmware never intended.
//     So the recon now SNAPSHOTS that block before its own reset and writes the FIRMWARE'S OWN values
//     back afterwards (`vendor_snapshot`/`vendor_restore`), re-running pad auto-calibration only if the
//     firmware had it enabled. Every value written there came from the controller one instruction
//     earlier; assumption 1 stands, with its scope stated precisely.
//  2. The CAPABILITIES base-clock field: if it reads 0 (some Tegra SKUs report base clock via the DT
//     `clock-frequency` / assigned-clock-rates instead of CAPS[15:8]), we assume a documented 200 MHz
//     and log it. Identification runs at 400 kHz then 25 MHz default-speed, so an inexact base only
//     changes the divider, never correctness.
//  3. Bus width / speed: identification runs 1-bit at default speed (<= 25 MHz). 4-bit / high-speed
//     negotiation (ACMD6 / CMD6) is deferred — it is not needed to census the card and keeps this rung
//     minimal. The reported width/speed is "1-bit, default-speed (25 MHz)".
//
// ## Read-only-by-construction (scope updated 2026-08-19 — the original absolute claim went stale)
//
// The RECON/IDENTIFY path issues ONLY the identification ladder + CMD17 single-block READ: CMD0,
// CMD8, CMD55/ACMD41, CMD2, CMD3, CMD9, CMD7, CMD16, CMD17 — no erase (CMD32–38) exists anywhere in
// this file, and no recon-path code can reach a write. Since ORIN-SDMMC-2/INSTALL-1 the file ALSO
// carries an explicit write path — `write_block_at` (CMD24, `sdmmc_arm`) and `write_blocks_at`
// (CMD25, `install_target`) — DOUBLE-GATED behind features the recon build does not carry, with no
// caller in the identify/census code. The controller-register writes recon makes (SRST, clock,
// power, command issue) are SDHCI machinery, not card-storage writes. An earlier version of this
// header claimed "grep for cmd(24) and find nothing" — that was true when written and is now false;
// the reachable-from-recon invariant is the one that holds (see review/unaos-orin-sdmmc1-LANDING.md).

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

// TEGRA-SDBLK (orin-unafs-root.md §3 item 1): the READ surface `drivers::block` consumes once the census
// has published the card as a block backend. Three functions, no more: the card's sector count, one
// single-sector read, and its counted loop. Nothing here can write — the write path stays exclusively
// behind the `sdmmc_arm` ladder further down this file, untouched by this seam.
#[cfg(feature = "tegra")]
pub use metal::{tegra_sd_card_blocks, tegra_sd_read_block_512, tegra_sd_read_blocks_512};

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
    const HOST_CTRL2: u64 = 0x3c; // [15:0] Auto-CMD error, [31:16] Host Control 2 (UHS mode / 1.8V sig)
    const HOST_VERSION: u64 = 0xfc; // [31:16] = Host Controller Version register (0xFE)

    // ── M2b: the Tegra vendor register block (>= 0x100), stacked above the standard SDHCI window (the
    //    controller window is 0x20000 long, so every offset here is in range). We do NOT author values
    //    for these — see `vendor_snapshot`/`vendor_restore`: SRST_ALL returns them to power-on defaults
    //    and thereby DISCARDS the pad tap/trim/drive-strength the bootloader calibrated for this exact
    //    card and board, which is the leading suspect for a ladder that dies with no card response after
    //    a reset that "succeeded". We preserve the firmware's values across our own reset; we never
    //    invent one. (Same register set Linux's sdhci-tegra re-applies in `tegra_sdhci_reset`.) ──
    const V_CLOCK_CTRL: u64 = 0x100; // tap/trim + PADPIPE/SPI clock-enable overrides
    const V_SYS_SW_CTRL: u64 = 0x104;
    const V_CAP_OVERRIDES: u64 = 0x10c;
    const V_MISC_CTRL: u64 = 0x120; // SDR50/DDR50/SDR104 capability enables
    const V_IO_TRIM_CTRL: u64 = 0x1ac;
    const V_DLLCAL_CFG: u64 = 0x1b0;
    const V_SDMEM_COMP_PADCTRL: u64 = 0x1e0; // pad comp / drive strength
    const V_AUTO_CAL_CONFIG: u64 = 0x1e4;
    const V_AUTO_CAL_STATUS: u64 = 0x1ec;
    /// The vendor registers preserved across SRST_ALL, in write order.
    const VENDOR_REGS: [u64; 8] = [
        V_CLOCK_CTRL,
        V_SYS_SW_CTRL,
        V_CAP_OVERRIDES,
        V_MISC_CTRL,
        V_IO_TRIM_CTRL,
        V_DLLCAL_CFG,
        V_SDMEM_COMP_PADCTRL,
        V_AUTO_CAL_CONFIG,
    ];
    const AUTO_CAL_START: u32 = 1 << 31;
    const AUTO_CAL_ENABLE: u32 = 1 << 29;
    const AUTO_CAL_ACTIVE: u32 = 1 << 31; // in V_AUTO_CAL_STATUS

    // ── Present State (0x24) bits. ──
    const ST_CMD_INHIBIT: u32 = 1 << 0;
    const ST_DAT_INHIBIT: u32 = 1 << 1;
    const ST_CARD_INSERTED: u32 = 1 << 16;

    // ── CONTROL1 (0x2C) bits. ──
    const C1_CLK_INTLEN: u32 = 1 << 0;
    const C1_CLK_STABLE: u32 = 1 << 1;
    const C1_CLK_EN: u32 = 1 << 2;
    const C1_SRST_HC: u32 = 1 << 24; // Software Reset For All (reg 0x2F bit 0)
    const C1_SRST_CMD: u32 = 1 << 25; // Software Reset For CMD line
    const C1_SRST_DAT: u32 = 1 << 26; // Software Reset For DAT line

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
    ///
    /// M2b: the `Err` payload is the INTERRUPT register AS READ AT THE FAILURE — a command that dies
    /// silently is a command that cannot be diagnosed from a boot capture (boot 3 stopped inside this
    /// ladder with no evidence whatsoever). Every caller that cares routes it through `cmd_step`, which
    /// names the command and prints the payload beside the Present State. The two `.is_err()` callers
    /// (the read/write primitives) ignore the payload exactly as before.
    fn send_command(base: u64, cmdtm: u32, arg: u32) -> Result<(), u32> {
        if !wait_clear(base, STATUS, ST_CMD_INHIBIT | ST_DAT_INHIBIT, CMD_TIMEOUT_MS) {
            return Err(read32(base, INTERRUPT));
        }
        write32(base, INTERRUPT, 0xffff_ffff); // clear any stale status
        write32(base, ARG1, arg);
        write32(base, CMDTM, cmdtm); // issues the command
        if !wait_set(base, INTERRUPT, INT_CMD_DONE | INT_ERR_ANY, CMD_TIMEOUT_MS) {
            return Err(read32(base, INTERRUPT));
        }
        let int = read32(base, INTERRUPT);
        if int & INT_ERR_ANY != 0 {
            write32(base, INTERRUPT, int); // W1C what we saw
            return Err(int);
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
    fn identify(base: u64, vendor_ok: bool) -> Option<Card> {
        // 0. M2b: snapshot the bootloader's Tegra vendor pad/tap configuration BEFORE the reset that
        //    would discard it, and report the pre-reset signalling voltage (a card the firmware left in
        //    1.8 V UHS signalling cannot answer a 3.3 V ladder — that diagnosis needs a PMIC/vqmmc arc,
        //    so we surface it rather than work around it).
        //    CLKPROOF gate (boot 4e): the HC2 read and the vendor block (base+0x100..0x1e4) run ONLY
        //    when the census proved the core clock enabled over BPMP — the 4e SError raised in this
        //    exact window. Unproven => the ladder runs pre-SDID-shaped (no vendor touch), which boot 3
        //    proved SError-free on this silicon; the skip is witnessed, never silent.
        let vendor = if vendor_ok {
            let hc2 = (read32(base, HOST_CTRL2) >> 16) & 0xffff;
            serial_println!(
                "{}   M2: pre-reset Host Control 2 = {:#06x} (1.8V-signalling={}, UHS mode {:#x}) ::",
                PS, hc2, (hc2 >> 3) & 1, hc2 & 0x7
            );
            Some(vendor_snapshot(base))
        } else {
            serial_println!(
                "{}   M2: vendor snapshot SKIPPED — core clock not proven (CLKPROOF); ladder runs without pad snapshot/restore ::",
                PS
            );
            None
        };

        // 1. Full-controller software reset; wait for it to self-clear.
        serial_println!("{}   M2: SRST_ALL (controller software reset) ::", PS);
        write32(base, CONTROL1, read32(base, CONTROL1) | C1_SRST_HC);
        if !wait_clear(base, CONTROL1, C1_SRST_HC, RESET_TIMEOUT_MS) {
            serial_println!("{}   M2: SRST did not self-clear (controller not responding) — STOP ::", PS);
            return None;
        }
        // 1b. M2b: put the firmware's vendor pad configuration BACK (SRST_ALL reset it to power-on
        //     defaults) and re-run pad auto-calibration if the firmware had it enabled. Skipped (with
        //     the M2 witness above) when CLKPROOF could not prove the clock.
        if let Some(v) = &vendor {
            vendor_restore(base, v);
        }
        // 2. Enable status latching (must be set or STATUS bits never appear in INTERRUPT); keep signals
        //    off (polled); clear any stale status.
        write32(base, IRPT_MASK, 0xffff_ffff);
        write32(base, IRPT_EN, 0);
        write32(base, INTERRUPT, 0xffff_ffff);
        // 3. Bus power, in the two writes the SDHCI spec prescribes (§3.3): FIRST select the bus voltage
        //    with power still off, THEN set SD Bus Power. A single fused write of bits[11:8] is what the
        //    recon did through boot 3, and a controller that latches the voltage only on a power-off
        //    write comes up unpowered from it — a silent way to reach CMD0 with a dead bus.
        let c0 = read32(base, CONTROL0) & !(0xf << 8);
        write32(base, CONTROL0, c0 | (0x7 << 9)); // 3.3 V select, SD Bus Power still 0
        write32(base, CONTROL0, c0 | (0xf << 8)); // ... then power on
        delay_ms(2); // rails settle before the first clock edge
        serial_println!(
            "{}   M2: bus power 3.3 V applied (CONTROL0={:#010x}) ::",
            PS, read32(base, CONTROL0)
        );

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

        serial_println!(
            "{}   M2: 400 kHz identification clock running (CONTROL1={:#010x}, base {} MHz) ::",
            PS, read32(base, CONTROL1), base_hz / 1_000_000
        );
        // 5b. The SD spec's power-up ramp: the card needs >= 1 ms and 74 SDCLK cycles (185 us at
        //     400 kHz) of clock before it will accept the first command. Boot 3 issued CMD0 in the
        //     instruction after CLK_STABLE — inside that window a card is not obliged to answer anything.
        delay_ms(2);

        // 6. CMD0 GO_IDLE (no response).
        cmd_step(base, "CMD0 GO_IDLE", cmd(0) | CMD_RESP_NONE, 0)?;
        delay_ms(2); // the card enters idle; give it the spec's settle before interrogating it
        // 7. CMD8 SEND_IF_COND (R7): 0x1AA = 2.7-3.6 V + check pattern 0xAA. The echo is the
        //    discriminator — and a NO-ANSWER is itself an answer: SD v1.x cards do not implement CMD8 at
        //    all, so the command TIMES OUT on them. Boot 3's ladder treated that timeout as fatal and
        //    abandoned identification; per SD Physical Layer §4.2.2 it means "not a v2 card, continue
        //    with ACMD41 and no HCS". The CMD line is reset after the timeout before we go on.
        let sdhc_capable = match send_command(base, cmd(8) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0x1aa) {
            Ok(()) => {
                let echo = read32(base, RESP0) & 0xfff;
                serial_println!("{}   M2: CMD8 SEND_IF_COND ok (R7 echo {:#05x}) ::", PS, echo);
                if echo != 0x1aa {
                    serial_println!("{}   M2: CMD8 echo mismatch — legacy/SDSC card (or no v2 support) ::", PS);
                }
                echo == 0x1aa
            }
            Err(int) => {
                serial_println!(
                    "{}   M2: CMD8 no response (INTERRUPT={:#010x}) — SD v1.x card or no v2 support; continuing without HCS ::",
                    PS, int
                );
                reset_cmd_dat(base);
                false
            }
        };
        // 8. ACMD41 loop (bounded ~1 s): CMD55 (APP_CMD) then ACMD41 (SD_SEND_OP_COND) with HCS + the
        //    3.3 V window, until power-up-busy (RESP0[31]) clears. ccs = RESP0[30].
        //    HCS (bit 30) is asserted ONLY when CMD8 was answered — a v1.x card must not be told the host
        //    supports high capacity. The loop paces itself (the card is allowed to stay busy for up to a
        //    second; hammering it back-to-back is not required and upsets some cards) and counts its
        //    rounds so the witness says how long power-up actually took.
        let acmd41_arg = if sdhc_capable { 0x40ff_8000 } else { 0x00ff_8000 };
        let acmd41_deadline = deadline_ms(ACMD41_TIMEOUT_MS);
        let mut ocr;
        let mut rounds = 0u32;
        loop {
            rounds += 1;
            let first = rounds == 1; // one witness for the first round; the rest are summarised below
            cmd_step_at(base, "CMD55 APP_CMD (before ACMD41)", cmd(55) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0, first)?;
            cmd_step_at(base, "ACMD41 SD_SEND_OP_COND", cmd(41) | CMD_RESP_48, acmd41_arg, first)?;
            ocr = read32(base, RESP0);
            if ocr & (1 << 31) != 0 {
                break;
            }
            if expired(acmd41_deadline) {
                serial_println!(
                    "{}   M2: ACMD41 power-up timed out after {} rounds (card never left busy, last OCR {:#010x}) — STOP ::",
                    PS, rounds, ocr
                );
                return None;
            }
            delay_ms(5);
        }
        let block_addressing = ocr & (1 << 30) != 0; // ccs
        serial_println!(
            "{}   M2: ACMD41 power-up complete in {} round(s) — OCR {:#010x} (CCS={}) ::",
            PS, rounds, ocr, block_addressing as u8
        );

        // 9. CMD2 ALL_SEND_CID (R2) -> identification state. Decode + print the CID.
        cmd_step(base, "CMD2 ALL_SEND_CID", cmd(2) | CMD_RESP_136 | CMD_CRCCHK, 0)?;
        let cid = read_resp(base);
        print_cid(&cid);

        // 10. CMD3 SEND_RELATIVE_ADDR (R6) -> rca in RESP0[31:16].
        cmd_step(base, "CMD3 SEND_RELATIVE_ADDR", cmd(3) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0)?;
        let rca = read32(base, RESP0) >> 16;
        let rca_arg = rca << 16;

        // 11. CMD9 SEND_CSD (R2) — card must be in stand-by (post-CMD3, pre-CMD7). Parse capacity.
        cmd_step(base, "CMD9 SEND_CSD", cmd(9) | CMD_RESP_136 | CMD_CRCCHK, rca_arg)?;
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
        cmd_step(base, "CMD7 SELECT_CARD", cmd(7) | CMD_RESP_48_BUSY | CMD_CRCCHK | CMD_IXCHK, rca_arg)?;
        // 13. CMD16 SET_BLOCKLEN 512 (R1; SDSC semantics, harmless on SDHC where 512 is fixed).
        cmd_step(base, "CMD16 SET_BLOCKLEN 512", cmd(16) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 512)?;
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
    // TEGRA-SDBLK — the READ surface the block layer consumes (orin-unafs-root.md §3 item 1)
    //
    // Until this section the recon was a closed loop: it identified the card, read sector 0, printed what
    // it saw, and told nobody. `drivers::block` could not reach the card at all — every SD arm in it is
    // `cfg(all(target_arch = "aarch64", feature = "baremetal"))`, i.e. the Pi's emmc2 — so on the Orin
    // `block::read_block` has always meant the USB stick and nothing else, and `unafs`'s
    // `SdSectorDevice::open()` could only answer `NoStorage`. This section is the seam that ends that,
    // and it is deliberately the SMALLEST one that can: a latched identity, a sector count, and a read.
    //
    // ### READ ONLY, and structurally so
    // Nothing below issues anything but CMD17. The write path (CMD24/CMD25) exists in this file only
    // behind the `sdmmc_arm` ladder and the `install_target` gate above it, and this section neither
    // calls into that ladder nor is callable from it. The block layer's own `write_*_tegra_sd` entry
    // points REFUSE unconditionally, in every cfg — so publishing the card as a block backend cannot,
    // by construction, add a writer to the Orin's card.
    //
    // ### Why a private CMD17 here instead of reusing `read_block_at`
    // `read_block_at` (the generalised single-block read) lives INSIDE the `sdmmc_arm` region and is
    // gated on it, because rung 2 deliberately kept the rung-1 read path untouched. Un-gating it would
    // edit the armed ladder for the convenience of an unarmed consumer, which is exactly the direction
    // that section's law forbids. `read_block_ro` below is therefore its unarmed twin: same command,
    // same bounded waits, same W1C discipline, reached only from this section. The duplication is the
    // price of leaving the ladder alone, and it is named rather than hidden.

    /// The published card identity — everything a sector read needs, as plain Copy data so it does not
    /// depend on `Card`'s `install_target`-gated derives. `None` until the census publishes (see
    /// `publish_block_backend`), which it only does after M1/M2/M3 have all succeeded.
    #[derive(Clone, Copy)]
    struct SdBlk {
        base: u64,
        block_addressing: bool,
        num_blocks: u64,
    }

    /// The one published card. The lock is held ACROSS each transfer, not merely to snapshot the
    /// identity: the SDHCI register file is a single shared resource, so two overlapping readers would
    /// interleave BLKSIZECNT/CMDTM/INTERRUPT writes on the same controller. Serialising here means the
    /// block layer above needs no rule of its own. Nothing in this file locks it re-entrantly.
    static SD_BLK: spin::Mutex<Option<SdBlk>> = spin::Mutex::new(None);

    /// Read one arbitrary block `lba` via polled single-block CMD17 into `buf` (512 bytes). READ-ONLY —
    /// the only card command it can issue is CMD17. Returns whether the block was read. The unarmed twin
    /// of `read_block_at` (see the section header for why it is not that function).
    fn read_block_ro(base: u64, block_addressing: bool, lba: u64, buf: &mut [u8; 512]) -> bool {
        // SDSC (byte addressing) can only express a 32-bit byte offset, so it tops out at 4 GiB; the
        // multiply is checked rather than wrapped, and an out-of-range LBA is a refusal, not a read of
        // some other sector.
        let arg = if block_addressing {
            if lba > u32::MAX as u64 {
                return false;
            }
            lba as u32
        } else {
            match lba.checked_mul(512) {
                Some(b) if b <= u32::MAX as u64 => b as u32,
                _ => return false,
            }
        };

        write32(base, INTERRUPT, 0xffff_ffff);
        write32(base, BLKSIZECNT, (1 << 16) | 512); // one block, 512 bytes
        if send_command(
            base,
            cmd(17) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK | CMD_ISDATA | CMD_DAT_DIR_READ,
            arg,
        )
        .is_err()
        {
            serial_println!("{}   SDBLK: CMD17 (READ LBA {}) failed at the link layer ::", PS, lba);
            return false;
        }
        let r1 = read32(base, RESP0);
        if r1 & R1_ERROR_MASK != 0 {
            serial_println!("{}   SDBLK: CMD17 LBA {} R1 error status {:#010x} ::", PS, lba, r1);
            return false;
        }
        if !wait_set(base, INTERRUPT, INT_READ_RDY, DATA_TIMEOUT_MS) {
            serial_println!("{}   SDBLK: LBA {} read buffer never became ready (READ_RDY timeout) ::", PS, lba);
            return false;
        }
        write32(base, INTERRUPT, INT_READ_RDY); // W1C
        for i in 0..128usize {
            let word = read32(base, DATA);
            let off = i * 4;
            buf[off..off + 4].copy_from_slice(&word.to_le_bytes());
        }
        if !wait_set(base, INTERRUPT, INT_DATA_DONE | INT_ERR_ANY, DATA_TIMEOUT_MS) {
            serial_println!("{}   SDBLK: LBA {} transfer-complete timeout after buffer read ::", PS, lba);
            return false;
        }
        let int = read32(base, INTERRUPT);
        write32(base, INTERRUPT, int); // W1C everything we saw
        if int & INT_ERR_ANY != 0 {
            serial_println!("{}   SDBLK: LBA {} data-transfer error status {:#010x} ::", PS, lba, int);
            return false;
        }
        true
    }

    /// The card's sector count as published to the block layer, or 0 if no card was published this boot.
    /// 0 is the honest "there is nothing here" answer — the block layer refuses to register a zero-sector
    /// device, and `unafs`'s size preference reads this the same way.
    pub fn tegra_sd_card_blocks() -> u64 {
        SD_BLK.lock().map(|c| c.num_blocks).unwrap_or(0)
    }

    /// Read one 512-byte sector into `buf` — the primitive `drivers::block::read_block_tegra_sd` calls.
    /// Bounds are re-checked HERE against the card's own capacity as well as at the block layer, because
    /// this function is what actually names an LBA to the card and must not depend on a caller's care.
    pub fn tegra_sd_read_block_512(lba: u64, buf: &mut [u8]) -> Result<usize, crate::drivers::block::BlockError> {
        use crate::drivers::block::BlockError;
        if buf.len() < 512 {
            return Err(BlockError::Io);
        }
        let guard = SD_BLK.lock();
        let card = guard.ok_or(BlockError::NotReady)?;
        if lba >= card.num_blocks {
            return Err(BlockError::BadLba);
        }
        let mut sec = [0u8; 512];
        if !read_block_ro(card.base, card.block_addressing, lba, &mut sec) {
            return Err(BlockError::Io);
        }
        buf[..512].copy_from_slice(&sec);
        Ok(512)
    }

    /// Read `count` consecutive sectors. This rung has no unarmed multi-block (CMD18) primitive — the
    /// counted read in this file is `install_target`-gated — so it LOOPS the proven CMD17, producing
    /// byte-for-byte the card traffic a per-sector caller would, in the same order. A CMD18 path can
    /// replace the body later with no change above the seam. A short read is an error, never a silent
    /// prefix: the one failure mode a filesystem above has no way to notice.
    pub fn tegra_sd_read_blocks_512(
        lba: u64,
        count: usize,
        buf: &mut [u8],
    ) -> Result<usize, crate::drivers::block::BlockError> {
        use crate::drivers::block::BlockError;
        if buf.len() < count * 512 {
            return Err(BlockError::Io);
        }
        for i in 0..count {
            let off = i * 512;
            tegra_sd_read_block_512(lba + i as u64, &mut buf[off..off + 512])?;
        }
        Ok(count * 512)
    }

    /// Publish the censused card to `drivers::block` as its own handle-less backend slot, and latch the
    /// identity the read functions above use.
    ///
    /// Called from exactly one place — the END of `sdmmc_census`, after M1 (window mapped, live SDHCI),
    /// M2 (card identified: CID/CSD/RCA, capacity decoded) and M3 (sector 0 actually read) have ALL
    /// succeeded. That ordering is the whole trust argument, and it is the same one SDHC-4b makes on
    /// x86: registration is the statement "a filesystem may trust this device", so it is only ever made
    /// about a card whose read path this boot has already exercised and reported on. Card-detect is not
    /// guessed at here — `identify` refuses an absent card, and M3 refuses one that cannot serve a read,
    /// so reaching this line IS the card-present proof.
    fn publish_block_backend(base: u64, card: &Card) {
        *SD_BLK.lock() = Some(SdBlk {
            base,
            block_addressing: card.block_addressing,
            num_blocks: card.num_blocks,
        });
        if !crate::drivers::block::register_tegra_sd(card.num_blocks, card.block_addressing) {
            // The block layer refused (zero sectors). Retract the latch rather than leave a read surface
            // pointing at a device the registry does not admit exists.
            *SD_BLK.lock() = None;
        }
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
            let root_raw = src.read_root().map_err(|_| InstallError::Io)?;
            // INSTALL-FILTER: drop host-OS litter trees at the ROOT level BEFORE sizing or cloning, so the
            // clone carries only the real boot payload and the verdict file count is honest. Only root
            // entries are filtered — nested payload dirs (EFI/BOOT/…) are copied exactly as before. One
            // witness line per skipped tree.
            let mut root_entries: alloc::vec::Vec<crate::fs::fat::DirEntry> = alloc::vec::Vec::new();
            for e in &root_raw {
                let name = e.name();
                if let Some(reason) = host_os_litter_reason(name) {
                    serial_println!(
                        "{}   INSTALL: skipped host-OS litter tree {} ({}) ::",
                        PS, name, reason
                    );
                    continue;
                }
                root_entries.push(*e);
            }
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
    /// INSTALL-FILTER: classify a ROOT directory entry as host-OS litter (a metadata/index tree the
    /// desktop OS drops on removable media — not part of the boot payload). Returns `Some(reason)` to
    /// SKIP the tree, `None` to clone it exactly as before.
    ///
    /// The in-tree FAT reader skips LFN slots (see `fs::fat`), so `name` is only ever the on-disk 8.3
    /// SHORT name (uppercase). We therefore match the 8.3 aliases the desktop OS's own FAT driver
    /// generates for these long names; the full long-name forms are listed too so intent is legible and
    /// the filter stays correct if the reader ever grows LFN awareness. Conservative by construction:
    /// only the exact known-litter names (plus the AppleDouble `._*`/`_~N` alias class) match — every
    /// other entry, including any real EFI/BOOT/KERNEL payload, is copied unchanged.
    #[cfg(feature = "install_target")]
    fn host_os_litter_reason(name: &str) -> Option<&'static str> {
        let eq = |a: &str| name.eq_ignore_ascii_case(a);
        // AppleDouble sidecars (`._<name>`): on 8.3 the alias collapses to the `_~N` class (e.g.
        // `_~8.TXT` seen on bench cards). Match the alias, and the literal `._` prefix for LFN-awareness.
        let up = name.as_bytes();
        let starts = |p: &[u8]| up.len() >= p.len() && up[..p.len()].eq_ignore_ascii_case(p);
        if starts(b"._") || starts(b"_~") {
            return Some("AppleDouble metadata sidecar (._* / 8.3 _~N alias)");
        }
        if eq("SPOTLI~1") || eq("SPOTLIGHT-V100") || eq(".SPOTLIGHT-V100") {
            return Some("macOS Spotlight index (Spotlight-V100)");
        }
        if eq("FSEVEN~1") || eq(".FSEVENTSD") {
            return Some("macOS FSEvents journal (.fseventsd)");
        }
        if eq("TRASHE~1") || eq(".TRASHES") {
            return Some("macOS trash (.Trashes)");
        }
        if eq("TEMPOR~1") || eq(".TEMPORARYITEMS") {
            return Some("macOS temporary items (.TemporaryItems)");
        }
        if eq("SYSTEM~1") || eq("SYSTEM VOLUME INFORMATION") {
            return Some("Windows System Volume Information");
        }
        None
    }

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
    fn resolve_microsd(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<(u64, u64, [u32; 4], usize)> {
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
        // The last rung of the M1 ladder, and the one that used to give up through a bare `?`: every
        // candidate above was logged, then a `None` here produced only the caller's generic
        // "no resolvable microSD-slot SDMMC controller" — with no line saying that candidates WERE
        // found and every one of them was status=disabled or reg-less. Name it (the SDID lesson).
        let Some(pick) = best.or(first_enabled) else {
            serial_println!(
                "{}   M1: {} SDMMC candidate(s) found but NONE is usable (need status=okay and a non-zero reg base; removable preferred) — STOP ::",
                PS, n
            );
            return None;
        };
        let _ = (pick.enabled, pick.removable); // (all fields consumed for logging clarity)
        let node = &pick.path[..pick.plen];
        serial_println!(
            "{}   M1: picked {} @ {:#010x} (size {:#x}) as the microSD slot ::",
            PS,
            core::str::from_utf8(node).unwrap_or("?"),
            pick.base, pick.size
        );
        // TEGRA-SD CLKPROOF: the picked node's `clocks` [phandle, id] pairs — the ids the BPMP
        // proof rung queries before the vendor block is touched. Read here because only this fn
        // holds the picked node path; odd-index words per the fdt pick convention.
        let clocks = fdt.prop_at(node, b"clocks");
        let mut clk_ids = [0u32; 4];
        let mut n_clks = 0usize;
        let mut ci = 1usize;
        while ci < clocks.n && n_clks < clk_ids.len() {
            clk_ids[n_clks] = clocks.words[ci];
            n_clks += 1;
            ci += 2;
        }
        Some((pick.base, pick.size, clk_ids, n_clks))
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
        let Some((base, size, clk_ids, n_clks)) = resolve_microsd(dtb_addr, dtb_size, ram_gib_mask) else {
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

        // ── TEGRA-SD CLKPROOF: prove the SDMMC core clock live over BPMP BEFORE identify() touches
        //    the vendor register block (base+0x100..0x1e4). Boot 4e died in an EL3 SError (async CBB
        //    abort, esr_el3=0xbe000011) inside vendor_snapshot's 8 reads — the first accesses this
        //    driver ever made beyond the standard SDHCI window, on an unproven clock/firewall domain.
        //    Pure MRQ query first (the JB7 precedent); CMD_CLK_ENABLE only if a clock reads off;
        //    every outcome named. If the proof cannot be completed the vendor block is SKIPPED with a
        //    witness (pre-SDID ladder behavior, metal-proven safe on boot 3) — never risked. ──
        let vendor_ok = 'proof: {
            if n_clks == 0 {
                serial_println!("{}   CLKPROOF: node carries no clocks property — vendor block will be SKIPPED ::", PS);
                break 'proof false;
            }
            let Some(geom) = crate::arch::aarch64::fdt_tegra::bpmp_geometry(dtb_addr, dtb_size, ram_gib_mask) else {
                serial_println!("{}   CLKPROOF: no BPMP geometry from DTB — vendor block will be SKIPPED ::", PS);
                break 'proof false;
            };
            let Some(chan) = crate::arch::aarch64::bpmp_tegra::jb1b_ping(&geom) else {
                serial_println!("{}   CLKPROOF: BPMP ping failed — vendor block will be SKIPPED ::", PS);
                break 'proof false;
            };
            let mut all_on = true;
            for i in 0..n_clks {
                let id = clk_ids[i];
                match crate::arch::aarch64::bpmp_tegra::clk_is_enabled(&chan, id) {
                    Some((0, state)) => {
                        serial_println!("{}   CLKPROOF: CLK {} IS_ENABLED = {} ::", PS, id, state);
                        if state == 0 {
                            match crate::arch::aarch64::bpmp_tegra::clk_enable(&chan, id) {
                                Some(0) => match crate::arch::aarch64::bpmp_tegra::clk_is_enabled(&chan, id) {
                                    Some((0, 1)) => {
                                        serial_println!("{}   CLKPROOF: CLK {} ENABLED + re-verified = 1 ::", PS, id)
                                    }
                                    other => {
                                        serial_println!("{}   CLKPROOF: CLK {} enable ack'd but re-query = {:?} — NOT proven ::", PS, id, other);
                                        all_on = false;
                                    }
                                },
                                other => {
                                    serial_println!("{}   CLKPROOF: CLK {} ENABLE -> {:?} — NOT proven ::", PS, id, other);
                                    all_on = false;
                                }
                            }
                        }
                    }
                    other => {
                        serial_println!("{}   CLKPROOF: CLK {} IS_ENABLED -> {:?} — NOT proven ::", PS, id, other);
                        all_on = false;
                    }
                }
            }
            if all_on {
                serial_println!("{}   CLKPROOF: all {} node clock(s) proven enabled ::", PS, n_clks);
            } else {
                serial_println!("{}   CLKPROOF: clock state NOT fully proven ::", PS);
            }
            // FWALL CONVICTION (metal, boots 4e + 5, 2026-08-21): the vendor range base+0x100..0x1e4
            // raises an async EL3 SError on this firmware EVEN WITH both DTB clocks proven enabled
            // (boot 5: CLK 120 = 1, CLK 219 = 1, then the identical esr_el3=0xbe000011 park at the
            // same rung). Clock gating is REFUTED; the surviving suspect is a firmware firewall/SCR
            // restriction on the vendor/pad-cal window. The block is therefore DISABLED outright —
            // pre-SDID shape, which boot 3 proved SError-free end-to-end — until a firmware-side
            // answer exists. CLKPROOF stays: read-only MRQ queries, and its lines are the standing
            // evidence that discriminates this conviction from clock gating on every future boot.
            serial_println!(
                "{}   FWALL: vendor block DISABLED — metal conviction: SError with clocks proven on (boots 4e+5); ladder runs without pad snapshot/restore ::",
                PS
            );
            let _ = all_on;
            false
        };

        // ── M2: SDHCI identification ladder (READ-ONLY) ──
        let Some(card) = identify(base, vendor_ok) else {
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

        // ── TEGRA-SDBLK: the card is now a block backend. LAST thing after every read witness (M1/M2/M3
        //    all passed above), and BEFORE the armed ladder / installer below — those two restore or
        //    rewrite CONTENT, never capacity, so the geometry published here is theirs as well, and a
        //    publish that waited for them would be a publish that never happened on an unarmed build. ──
        publish_block_backend(base, &card);

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

    // ══════════════════════════════════════════════════════════════════════════════════════════════════
    // M2b — what boot 3 was missing. The identification ladder above was complete as SD-spec prose, and
    // it still ended at `recon done at M2 (no identified card / honest stop)` with NOTHING between the
    // card-detect line and the stop. Three defects, in the order they bite:
    //
    //  1. NO EVIDENCE. Every rung was `send_command(..).ok()?` — a failure returned `None` through the
    //     `?` without a single serial line, so a capture could not say which command died, nor with
    //     which error bits. `cmd_step` below is the fix: it names the command and prints the INTERRUPT
    //     payload beside the Present State, then resets the CMD/DAT lines so the controller is not left
    //     inhibited for whatever runs next.
    //  2. NO SETTLE TIME. CMD0 was issued in the instruction after CLK_STABLE, and CMD8 in the one after
    //     CMD0. The SD power-up ramp requires >= 1 ms plus 74 SDCLK cycles of clock before the card
    //     accepts anything (`delay_ms` + the two 2 ms waits in the ladder).
    //  3. NO VENDOR PRESERVATION. SRST_ALL returns the Tegra vendor block (>= 0x100) to power-on
    //     defaults, discarding the tap/trim/drive-strength/auto-cal the bootloader calibrated for this
    //     board — after which the pads may not sustain a command the card will answer. `vendor_snapshot`
    //     / `vendor_restore` put the FIRMWARE'S OWN values back; they author none.
    //
    // Plus the one outright spec bug: CMD8 timing out is how an SD v1.x card says "I am not v2", and the
    // old ladder took it as fatal. That is handled inline at rung 7, not here.
    //
    // Read-only is untouched: nothing in this section issues any card command at all.

    /// Bounded busy-wait of `ms` milliseconds on the monotonic counter (same discipline as `deadline_ms`;
    /// there is no sleeping scheduler at this point in boot).
    fn delay_ms(ms: u64) {
        let dl = deadline_ms(ms);
        while !expired(dl) {
            core::hint::spin_loop();
        }
    }

    /// Reset just the CMD and DAT lines (NOT SRST_ALL — that would discard the vendor configuration and
    /// the bus power we set up). This is what the SDHCI spec prescribes after a command timeout, and it
    /// is what lets the ladder continue past a CMD8 that no v1.x card will ever answer.
    fn reset_cmd_dat(base: u64) {
        write32(base, CONTROL1, read32(base, CONTROL1) | C1_SRST_CMD | C1_SRST_DAT);
        if !wait_clear(base, CONTROL1, C1_SRST_CMD | C1_SRST_DAT, RESET_TIMEOUT_MS) {
            serial_println!("{}   M2: CMD/DAT line reset did not self-clear (controller wedged) ::", PS);
        }
        write32(base, INTERRUPT, 0xffff_ffff);
    }

    /// Issue one NAMED identification command, witnessing both outcomes. `None` (which `?` turns into the
    /// ladder's honest stop) is only ever returned AFTER a line saying exactly which command failed and
    /// what the controller had latched — the thing boot 3 could not tell us.
    fn cmd_step(base: u64, name: &str, cmdtm: u32, arg: u32) -> Option<()> {
        cmd_step_at(base, name, cmdtm, arg, true)
    }

    /// `cmd_step` with the success line suppressed — for the ACMD41 poll, whose rounds are summarised
    /// once by the loop instead of one line per round.
    fn cmd_step_at(base: u64, name: &str, cmdtm: u32, arg: u32, verbose: bool) -> Option<()> {
        match send_command(base, cmdtm, arg) {
            Ok(()) => {
                if verbose {
                    serial_println!("{}   M2: {} ok ::", PS, name);
                }
                Some(())
            }
            Err(int) => {
                serial_println!(
                    "{}   M2: {} FAILED — INTERRUPT={:#010x} PresentState={:#010x} CONTROL0={:#010x} CONTROL1={:#010x} — STOP ::",
                    PS, name, int,
                    read32(base, STATUS), read32(base, CONTROL0), read32(base, CONTROL1)
                );
                reset_cmd_dat(base);
                None
            }
        }
    }

    /// Snapshot the Tegra vendor register block so SRST_ALL cannot lose the bootloader's calibration.
    /// Purely a read; logged so a capture records what the firmware had configured.
    fn vendor_snapshot(base: u64) -> [u32; VENDOR_REGS.len()] {
        let mut vals = [0u32; VENDOR_REGS.len()];
        for (i, off) in VENDOR_REGS.iter().enumerate() {
            vals[i] = read32(base, *off);
        }
        serial_println!(
            "{}   M2: vendor block pre-reset — CLOCK_CTRL={:#010x} SYS_SW={:#010x} CAP_OVR={:#010x} MISC={:#010x} IO_TRIM={:#010x} DLLCAL={:#010x} COMP_PAD={:#010x} AUTO_CAL={:#010x} ::",
            PS, vals[0], vals[1], vals[2], vals[3], vals[4], vals[5], vals[6], vals[7]
        );
        vals
    }

    /// Put the snapshot back after SRST_ALL, then re-run pad auto-calibration IF the firmware had it
    /// enabled (calibration results do not survive the reset either). Bounded wait on AUTO_CAL_ACTIVE;
    /// a calibration that never finishes is reported, never waited on forever, and never fatal — the
    /// ladder proceeds and the command witnesses will say whether the bus works.
    fn vendor_restore(base: u64, vals: &[u32; VENDOR_REGS.len()]) {
        let poisoned = vals.iter().all(|v| is_poison(*v));
        if poisoned {
            serial_println!(
                "{}   M2: vendor block read back all-poison — NOT restoring (the window would not be a live vendor block) ::",
                PS
            );
            return;
        }
        let mut skipped = 0u32; // per-register guard: a PARTIAL poison read must not author 0xffffffff into tap/trim (delta-review hardening)
        for (i, off) in VENDOR_REGS.iter().enumerate() {
            if is_poison(vals[i]) {
                skipped += 1;
                continue;
            }
            write32(base, *off, vals[i]);
        }
        if skipped > 0 {
            serial_println!(
                "{}   M2: vendor block restored across SRST_ALL — {} poison register(s) SKIPPED (left at reset defaults, none authored) ::",
                PS, skipped
            );
        } else {
        serial_println!("{}   M2: vendor block restored across SRST_ALL (firmware values, none authored) ::", PS);
        }

        if is_poison(vals[7]) || vals[7] & AUTO_CAL_ENABLE == 0 {
            serial_println!("{}   M2: pad auto-calibration was disabled by firmware — not started ::", PS);
            return;
        }
        write32(base, V_AUTO_CAL_CONFIG, vals[7] | AUTO_CAL_ENABLE | AUTO_CAL_START);
        if !wait_clear(base, V_AUTO_CAL_STATUS, AUTO_CAL_ACTIVE, RESET_TIMEOUT_MS) {
            serial_println!(
                "{}   M2: pad auto-calibration still ACTIVE after {} ms (status {:#010x}) — continuing with the restored values ::",
                PS, RESET_TIMEOUT_MS, read32(base, V_AUTO_CAL_STATUS)
            );
            return;
        }
        serial_println!(
            "{}   M2: pad auto-calibration complete (AUTO_CAL_CONFIG={:#010x}, status={:#010x}) ::",
            PS, read32(base, V_AUTO_CAL_CONFIG), read32(base, V_AUTO_CAL_STATUS)
        );
    }
}
