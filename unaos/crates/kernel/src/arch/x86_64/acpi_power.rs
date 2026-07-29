// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ACPI S5 soft-off: turn the machine OFF, rather than parking the CPU in a `hlt` loop.
//
// Motivation (attended bench boot, 2012 rMBP): the graphical installer's `q` = halt stopped the
// kernel but left the laptop powered, fans running, battery draining, and the only exit a
// five-second power-button hold. "Halt" on a machine that can be turned off should mean off.
//
// The mechanism, in ACPI's terms (spec §7.4, §16.1): sleeping state S5 ("soft off") is entered by
// writing a firmware-chosen 3-bit value `SLP_TYP` into the PM1 control register together with
// `SLP_EN`. Two facts are needed and NEITHER may be guessed:
//
//   1. WHERE the PM1 control register lives — the FADT's `PM1a_CNT_BLK` / `PM1b_CNT_BLK` (or
//      their ACPI 2.0+ Generic-Address-Structure twins `X_PM1a_CNT_BLK` / `X_PM1b_CNT_BLK`).
//      Chipset-specific: 0x604 on QEMU's ICH9/PIIX4, 0x1804 on this rMBP's Series-7 PCH.
//   2. WHAT value means "off" — the `SLP_TYPa` / `SLP_TYPb` pair, which lives nowhere in a
//      fixed table. It is *bytecode*: firmware publishes a `\_S5_` Name object in the DSDT
//      whose value is a Package of the per-PM1-block sleep types. 5 is common, 7 exists,
//      0 exists. Guessing means writing an arbitrary 3-bit code into a live chipset power
//      register — which on the wrong machine selects S1/S3/S4 instead, or a reserved encoding.
//
// So this module refuses rather than improvises. Every step that can fail prints one honest
// witness line naming what it could not determine, and the caller falls back to the pre-existing
// `hlt` loop. The machine staying on is a disappointment; a blind write to a power-management
// register is a hazard, and the difference is the whole point of the STOP-tripwire discipline.
//
// The `_S5_` scan is a deliberately narrow byte-pattern probe, not an AML interpreter. UnaOS has
// no AML machine and this arc is not where one gets written; what it has is a recogniser for the
// one encoding shape that essentially every firmware emits for this particular object. The
// accepted and refused shapes are enumerated exactly at `scan_s5` — read that list as the
// module's contract, because anything outside it takes the fallback rather than a guess.

use super::acpi;
use x86_64::instructions::port::Port;

// --- FADT field offsets (ACPI spec; stable since 1.0 — later revisions only append) ---

/// u32: physical address of the DSDT (the AML table that carries `\_S5_`). Superseded by
/// `X_DSDT` when the FADT is long enough and that field is populated.
const FADT_DSDT: usize = 40;
/// u32: I/O port of the SMI command register, used to hand the platform the "ACPI mode, please"
/// request. 0 = the platform has no SMI-based ACPI enable (it is either always-on or legacy-only).
const FADT_SMI_CMD: usize = 48;
/// u8: the value to write to `SMI_CMD` to request ACPI mode. 0 = not supported.
const FADT_ACPI_ENABLE: usize = 52;
/// u32: I/O port of the PM1a control register block (the required one).
const FADT_PM1A_CNT_BLK: usize = 64;
/// u32: I/O port of the PM1b control register block (optional; 0 when the chipset has only PM1a).
const FADT_PM1B_CNT_BLK: usize = 68;
/// u64: 64-bit DSDT address (ACPI 2.0+).
const FADT_X_DSDT: usize = 140;
/// 12-byte Generic Address Structure: extended PM1a control block (ACPI 2.0+); preferred.
const FADT_X_PM1A_CNT_BLK: usize = 172;
/// 12-byte Generic Address Structure: extended PM1b control block (ACPI 2.0+); preferred.
const FADT_X_PM1B_CNT_BLK: usize = 184;
/// Generic Address Structure `AddressSpaceId` for System I/O space — the only space this module
/// accepts for a PM1 block. A memory-space PM1 block is legal in the spec and appears on some
/// reduced-hardware platforms; we refuse it rather than pretend the `out` instruction reaches it.
const GAS_SPACE_SYSTEM_IO: u8 = 1;

// --- PM1 control register bits (ACPI §4.8.3.2.1) ---

/// SCI_EN (bit 0): 1 = the platform is in ACPI mode and PM1 writes are honoured; 0 = legacy
/// mode, where the chipset may ignore a sleep request entirely.
const PM1_CNT_SCI_EN: u16 = 1 << 0;
/// SLP_TYP occupies bits 10..12 — a 3-bit firmware-defined sleeping-state selector.
const PM1_CNT_SLP_TYP_SHIFT: u16 = 10;
/// SLP_EN (bit 13): write-only trigger. Setting it commits the transition to the state named by
/// SLP_TYP. This is the bit that ends the machine.
const PM1_CNT_SLP_EN: u16 = 1 << 13;

/// Everything needed to execute the soft-off, once every piece has been *discovered* rather than
/// assumed. Construction of this struct is the proof that no field was guessed.
#[derive(Clone, Copy)]
pub struct S5 {
    /// PM1a control register I/O port (always present — the spec requires PM1a).
    pm1a: u16,
    /// PM1b control register I/O port, or 0 when the chipset has no second block.
    pm1b: u16,
    /// SLP_TYP value for PM1a, from `\_S5_` element 0.
    slp_typa: u8,
    /// SLP_TYP value for PM1b, from `\_S5_` element 1.
    slp_typb: u8,
    /// SMI command port and ACPI-enable value, for the legacy-to-ACPI-mode handshake.
    smi_cmd: u16,
    acpi_enable: u8,
}

/// Read a 12-byte Generic Address Structure and return its port if — and only if — it names a
/// non-zero System I/O address that fits in 16 bits. Any other space id (system memory, PCI
/// config, embedded controller) or an empty/oversized address yields `None`, which sends the
/// caller to the legacy 32-bit field rather than to a fabricated port.
///
/// SAFETY: `gas` must point at 12 mapped bytes inside an ACPI table.
unsafe fn gas_io_port(gas: usize) -> Option<u16> {
    let space = (gas as *const u8).read_unaligned();
    let addr = ((gas + 4) as *const u64).read_unaligned();
    if space == GAS_SPACE_SYSTEM_IO && addr != 0 && addr <= 0xFFFF {
        Some(addr as u16)
    } else {
        None
    }
}

/// Discover the PM1 control ports and the S5 sleep types, or return the name of the first step
/// that could not be completed. The returned `&'static str` is the witness text: it says which
/// fact was missing, never "something went wrong".
fn discover() -> Result<S5, &'static str> {
    let rsdp = acpi::rsdp_addr();
    let (sdt_addr, entry_size) = acpi::root_sdt(rsdp).ok_or("no RSDP / bad RSDP signature")?;

    // SAFETY: all firmware tables are identity-mapped by the bootloader; every access below is a
    // read of a byte-packed table field through `read_unaligned`, bounded by the table's own
    // `length`. No write happens anywhere in this function.
    unsafe {
        let fadt = acpi::find_table(sdt_addr, entry_size, b"FACP").ok_or("no FADT (FACP) table")?;
        let fadt_len = acpi::table_len(fadt);

        // --- PM1a control port: prefer the ACPI 2.0+ GAS, fall back to the legacy u32 field ---
        let mut pm1a: Option<u16> = None;
        if fadt_len >= FADT_X_PM1A_CNT_BLK + 12 {
            pm1a = gas_io_port(fadt as usize + FADT_X_PM1A_CNT_BLK);
        }
        if pm1a.is_none() && fadt_len >= FADT_PM1A_CNT_BLK + 4 {
            let legacy = ((fadt as usize + FADT_PM1A_CNT_BLK) as *const u32).read_unaligned();
            if legacy != 0 && legacy <= 0xFFFF {
                pm1a = Some(legacy as u16);
            }
        }
        let pm1a = pm1a.ok_or("FADT names no PM1a_CNT_BLK in System I/O space")?;

        // --- PM1b control port: genuinely optional; 0 means "this chipset has one block" ---
        let mut pm1b: u16 = 0;
        if fadt_len >= FADT_X_PM1B_CNT_BLK + 12 {
            pm1b = gas_io_port(fadt as usize + FADT_X_PM1B_CNT_BLK).unwrap_or(0);
        }
        if pm1b == 0 && fadt_len >= FADT_PM1B_CNT_BLK + 4 {
            let legacy = ((fadt as usize + FADT_PM1B_CNT_BLK) as *const u32).read_unaligned();
            if legacy != 0 && legacy <= 0xFFFF {
                pm1b = legacy as u16;
            }
        }

        // --- SMI handshake parameters (both optional; 0 = no SMI-based ACPI enable) ---
        let smi_cmd = if fadt_len >= FADT_SMI_CMD + 4 {
            let v = ((fadt as usize + FADT_SMI_CMD) as *const u32).read_unaligned();
            if v <= 0xFFFF { v as u16 } else { 0 }
        } else {
            0
        };
        let acpi_enable = if fadt_len >= FADT_ACPI_ENABLE + 1 {
            ((fadt as usize + FADT_ACPI_ENABLE) as *const u8).read_unaligned()
        } else {
            0
        };

        // --- SLP_TYPa / SLP_TYPb from the AML `\_S5_` package ---
        //
        // The FADT points at the DSDT, which is where `_S5_` normally lives. But a Name object is
        // legal in any AML table, and vendors (Apple included) do split their namespace across
        // SSDTs, so the DSDT is searched first and every SSDT after it. First recognisable match
        // wins — duplicate `_S5_` definitions would be a firmware bug, not a case to arbitrate.
        let mut dsdt: u64 = 0;
        if fadt_len >= FADT_X_DSDT + 8 {
            dsdt = ((fadt as usize + FADT_X_DSDT) as *const u64).read_unaligned();
        }
        if dsdt == 0 && fadt_len >= FADT_DSDT + 4 {
            dsdt = ((fadt as usize + FADT_DSDT) as *const u32).read_unaligned() as u64;
        }

        let mut types = if dsdt != 0 { scan_s5(dsdt) } else { None };
        if types.is_none() {
            acpi::each_table(sdt_addr, entry_size, |addr, sig| {
                if sig == b"SSDT" {
                    types = scan_s5(addr);
                }
                types.is_some()
            });
        }
        let (slp_typa, slp_typb) =
            types.ok_or("no recognisable \\_S5_ package in DSDT or any SSDT")?;

        Ok(S5 { pm1a, pm1b, slp_typa, slp_typb, smi_cmd, acpi_enable })
    }
}

/// Scan one AML table (DSDT or SSDT) for a `\_S5_` Name whose value is a Package, and return
/// `(SLP_TYPa, SLP_TYPb)` from its first two elements.
///
/// This is a byte-pattern recogniser over the table body — NOT an AML interpreter. It handles
/// exactly the following shapes and refuses everything else by returning `None`:
///
/// ACCEPTED:
///   * `08 5F 53 35 5F` — `NameOp` + the NameSeg `_S5_` (the overwhelmingly common encoding).
///   * `08 5C 5F 53 35 5F` — `NameOp` + `RootChar` (`\`) + `_S5_`, i.e. an explicitly rooted
///     `\_S5_`, which some vendors emit.
///   followed by `12` (`PackageOp`), a **single-byte** PkgLength (top two bits of the length
///   byte clear — always true for a package this small), a NumElements byte of 2 or more, and
///   then two elements each of which is one of:
///     - `00` (`ZeroOp`)          -> 0
///     - `01` (`OneOp`)           -> 1
///     - `0A xx` (`BytePrefix`)   -> xx
///   Both resulting SLP_TYP values must fit the register's 3-bit field (<= 7).
///
/// REFUSED (fall back to the `hlt` loop, no write):
///   * `13` `VarPackageOp` instead of `12` — its size is a runtime expression, so reading it
///     without an interpreter would be a guess.
///   * Multi-byte PkgLength encodings (length byte with either high bit set).
///   * Any element that is not Zero/One/BytePrefix — `0B` WordPrefix, `0C` DWordPrefix, a
///     NameString reference to another object, a Buffer, or a nested Package.
///   * `_S5_` reached only through an Alias, produced by a Method, or otherwise not introduced
///     by a literal `NameOp` at the matched offset — the four ASCII bytes appear, but without
///     the `08` (or `08 5C`) prefix we do not claim to know what object they belong to.
///   * A package declaring fewer than 2 elements, or one whose element bytes run past the end
///     of the table.
///   * Any SLP_TYP value above 7.
///
/// The scan is read-only and bounded by the table's own `length` field, so a malformed table
/// costs a refusal, not a fault.
///
/// SAFETY: `table_addr` must point at a mapped ACPI table with a valid 36-byte header.
unsafe fn scan_s5(table_addr: u64) -> Option<(u8, u8)> {
    let len = acpi::table_len(table_addr);
    if len <= acpi::SDT_HEADER_LEN {
        return None;
    }
    let bytes = core::slice::from_raw_parts(table_addr as *const u8, len);
    let body = &bytes[acpi::SDT_HEADER_LEN..];

    // Find each occurrence of the NameSeg "_S5_" and try to parse a package behind it. A table
    // can legitimately contain those four bytes inside a string or another NameSeg (e.g. a
    // method named `_S5_` we must not touch), hence "try, and keep looking" rather than
    // "first hit or bust".
    let mut i = 0usize;
    while i + 4 <= body.len() {
        if &body[i..i + 4] != b"_S5_" {
            i += 1;
            continue;
        }
        // Require a literal NameOp introducer, optionally with a RootChar between it and the
        // NameSeg. Anything else and we do not claim to understand this occurrence.
        let introduced = (i >= 1 && body[i - 1] == 0x08)
            || (i >= 2 && body[i - 2] == 0x08 && body[i - 1] == 0x5C);
        if introduced {
            if let Some(types) = parse_s5_package(&body[i + 4..]) {
                return Some(types);
            }
        }
        i += 1;
    }
    None
}

/// Parse the `PackageOp` that should follow a `_S5_` NameSeg. `rest` starts at the byte
/// immediately after the four NameSeg bytes. See `scan_s5` for the exact accepted grammar.
fn parse_s5_package(rest: &[u8]) -> Option<(u8, u8)> {
    // PackageOp (0x12). VarPackageOp (0x13) is deliberately not accepted.
    if rest.first().copied()? != 0x12 {
        return None;
    }
    // PkgLength: only the single-byte form (bits 6-7 of the lead byte clear). A `_S5_` package is
    // on the order of eight bytes, so the multi-byte forms would themselves be a sign that this
    // is not the object we think it is.
    let pkg_lead = rest.get(1).copied()?;
    if pkg_lead & 0xC0 != 0 {
        return None;
    }
    // NumElements. Two are needed (PM1a's and PM1b's sleep type); more are normal — ACPI 1.0
    // packages carried a third/fourth reserved element — and are simply ignored.
    if rest.get(2).copied()? < 2 {
        return None;
    }

    // Two consecutive constant elements, each Zero / One / BytePrefix.
    let mut at = 3usize;
    let mut read_const = || -> Option<u8> {
        let op = rest.get(at).copied()?;
        match op {
            0x00 => {
                at += 1;
                Some(0)
            }
            0x01 => {
                at += 1;
                Some(1)
            }
            0x0A => {
                let v = rest.get(at + 1).copied()?;
                at += 2;
                Some(v)
            }
            _ => None,
        }
    };
    let a = read_const()?;
    let b = read_const()?;

    // SLP_TYP is a 3-bit field. A value that does not fit is not a value we understand, and
    // truncating it would be exactly the guess this module exists to avoid.
    if a > 7 || b > 7 {
        return None;
    }
    Some((a, b))
}

/// Put the platform into ACPI mode if it is not already there, so that a PM1 sleep request is
/// actually honoured rather than dropped by a chipset still in legacy mode.
///
/// Returns the SCI_EN state observed afterwards, for the witness line. This is *not* treated as a
/// hard failure: on a UEFI boot the firmware has almost always enabled ACPI for us already, and
/// on the platforms where SCI_EN reads back clear the sleep write is frequently still honoured
/// (QEMU's PM device acts on SLP_EN unconditionally). The witness records what we saw so a serial
/// capture can distinguish "wrote into legacy mode and nothing happened" from "port was wrong".
///
/// SAFETY: writes the firmware-declared `ACPI_ENABLE` value to the firmware-declared `SMI_CMD`
/// port — both straight out of the FADT, neither invented here.
unsafe fn enter_acpi_mode(s5: &S5) -> bool {
    let mut pm1a: Port<u16> = Port::new(s5.pm1a);
    if pm1a.read() & PM1_CNT_SCI_EN != 0 {
        return true;
    }
    if s5.smi_cmd == 0 || s5.acpi_enable == 0 {
        return false; // platform declares no SMI handshake; nothing honest left to try
    }
    Port::<u8>::new(s5.smi_cmd).write(s5.acpi_enable);
    // The SMI is synchronous in practice but the spec allows the transition to take time; poll a
    // bounded number of times rather than spinning forever on a machine that will never comply.
    for _ in 0..100_000 {
        if pm1a.read() & PM1_CNT_SCI_EN != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Power the machine off via ACPI S5, or — if any required fact could not be discovered — print
/// one witness line saying which one and park the CPU in the pre-existing `hlt` loop.
///
/// Never returns: either the machine loses power mid-instruction, or we fall through to `hlt`.
/// The witness line is emitted *before* the register write precisely because a successful soft-off
/// kills the machine part-way through the next line; seeing the `:: ACPI: S5 poweroff ... ::`
/// marker as the last thing on the serial capture is the proof that this path executed.
pub fn poweroff() -> ! {
    let s5 = match discover() {
        Ok(s5) => s5,
        Err(why) => {
            serial_println!(":: ACPI: S5 unavailable — {} — parking in hlt instead ::", why);
            crate::hlt_loop();
        }
    };

    // SAFETY: every port and value below came out of the firmware's own tables (FADT for the
    // ports, the DSDT/SSDT `\_S5_` package for the sleep types); nothing is invented. Interrupts
    // are masked first so no handler can run between the witness line and the write, and so the
    // two PM1 blocks are programmed without a timer tick in between.
    unsafe {
        x86_64::instructions::interrupts::disable();
        let sci_en = enter_acpi_mode(&s5);

        serial_println!(
            ":: ACPI: S5 poweroff slp_typa={:#x} slp_typb={:#x} pm1a={:#x} pm1b={:#x} sci_en={} ::",
            s5.slp_typa,
            s5.slp_typb,
            s5.pm1a,
            s5.pm1b,
            sci_en as u8
        );

        // PM1 control is a read-modify-write register: the other bits (SCI_EN, BM_RLD, GBL_RLS)
        // belong to the platform and must survive. Clear only the SLP_TYP field, insert ours, and
        // set SLP_EN — the write of SLP_EN is what commits the transition.
        let mut pm1a: Port<u16> = Port::new(s5.pm1a);
        let val = (pm1a.read() & !(0x7 << PM1_CNT_SLP_TYP_SHIFT))
            | ((s5.slp_typa as u16 & 0x7) << PM1_CNT_SLP_TYP_SHIFT)
            | PM1_CNT_SLP_EN;
        pm1a.write(val);

        // On a split PM1 implementation the transition only completes once *both* blocks have
        // been written; on the single-block chipsets pm1b is 0 and this is skipped.
        if s5.pm1b != 0 {
            let mut pm1b: Port<u16> = Port::new(s5.pm1b);
            let val = (pm1b.read() & !(0x7 << PM1_CNT_SLP_TYP_SHIFT))
                | ((s5.slp_typb as u16 & 0x7) << PM1_CNT_SLP_TYP_SHIFT)
                | PM1_CNT_SLP_EN;
            pm1b.write(val);
        }
    }

    // Reaching here means the write was accepted by the port but the platform did not act — the
    // sleep types were right in shape but wrong for this chipset, or the chipset was still in
    // legacy mode. Say so once, then park exactly as the pre-S5 halt path did.
    serial_println!(":: ACPI: S5 write returned — platform did not power off; parking in hlt ::");
    crate::hlt_loop();
}
