#![no_std]
#![no_main]

//! orin-xhci-repro — a standalone UEFI reproducer for the Tegra234/Jetson Orin
//! xHCI-takeover RAS carveout fault reported to NVIDIA as developer forum topic 377113:
//! <https://forums.developer.nvidia.com/t/orin-nano-r39-2-ras-carveout-fault-on-crcr-write-to-uefi-inherited-xhci-identical-binary-faults-one-day-boots-clean-the-next/377113>
//!
//! WHAT THIS IS: a UEFI application that runs standalone, on stock UEFI, with NO UnaOS
//! kernel and no UnaOS build tooling. It requires nothing but a UEFI boot stick carrying
//! this .efi binary. It is READ-ONLY except for exactly one gated register write.
//!
//! THE TRIGGER (traced from UnaOS's own kernel source, not re-derived here — see the
//! comment above `main()` for the exact provenance): on the affected platform, some
//! boots have a controller-internal "poisoned" latch such that the FIRST time anything
//! engages the xHCI command-ring register machinery on the UEFI-inherited, still-RUNNING
//! (USBCMD.RS=1) controller, the platform's SNOC/ACI fabric issues a FillWrite into a
//! protected carveout and the box RAS-power-offs. UnaOS's own bench work isolated this to
//! a single 64-bit write to the CRCR register (xHCI operational-register offset 0x18)
//! issued while RS=1 — fired 2/2 deterministic on the boots where it was tested this way.
//! That validated trigger write carried a real, non-null, freshly-allocated 64-bit command
//! ring pointer with RCS=1 set (the shape `ring_address | 1`) — never a null/zero pointer.
//! The same underlying fault has also fired historically at a later point in the boot (a
//! doorbell ring, a different register entirely) across many different builds with
//! different heap layouts and therefore different ring addresses, which is why the working
//! theory is "engaging the command-ring register machinery, not one specific pointer
//! value" — but that theory has NOT been tested against a NULL-pointer write specifically.
//! This app reproduces the VALIDATED shape as closely as a standalone tool can: it writes a
//! synthetic,
//! plausible-looking, 64-byte-aligned, non-null pointer (with RCS=1 set) to CRCR — chosen
//! to match the shape of the validated trigger writes, not an echo of the always-reads-
//! zero register (per xHCI 5.4.5 a CRCR read never returns the pointer field, so echoing
//! the read value back would write a NULL pointer — a real, if inert, state change, and a
//! materially different write than either validated instance). This app never rings a
//! doorbell, so the synthetic pointer is never dereferenced by the controller; only its
//! bit pattern in the register is exercised.
//!
//! WHAT THIS APP DELIBERATELY DOES NOT DO: it does not take over the controller, does not
//! halt it, does not reprogram DCBAAP/ERST/CONFIG, does not build a command ring, and
//! rings no doorbell. UnaOS's kernel takeover does all of that before its own CRCR
//! engagement; this app skips every bit of it and still expects the fault, because the
//! named trigger is the bare register write on the controller AS UEFI ITSELF LEFT IT
//! running — no takeover should be necessary to observe it. If NVIDIA's engineers find
//! that the fault does NOT reproduce without the fuller takeover sequence, that is itself
//! important data worth reporting back.
//!
//! EXPECTED RESULT ON AN AFFECTED/POISONED BOOT: an immediate RAS power-off. QEMU cannot
//! model this fault (there is no Tegra234 RAS/SNOC/ACI model) — this app has only ever
//! been exercised by inspection and compilation, never fault-observed in the loop; the
//! actual fault observation is an attended metal run, and as of this writing that run has
//! not yet happened. Treat this exact tool as an unverified-on-hardware best effort until
//! that first metal run confirms or corrects it.

use core::time::Duration;
use log::{error, info, warn};
use uefi::prelude::*;
use uefi::proto::console::text::Key;
use uefi::{boot, system, Char16};

/// Tegra234 XUSB/xHCI host-controller MMIO base.
///
/// This is a FIXED platform constant, not something either UnaOS or this app discovers
/// dynamically (there is no PCIe config space to enumerate here — the controller is a
/// directly memory-mapped SoC block). It is the same address as:
///   - the Tegra234 device-tree binding node `usb@3610000` (Linux's `tegra234.dtsi`),
///   - `XUSB_HOST` in UnaOS's own kernel takeover
///     (`unaos/crates/kernel/src/arch/aarch64/xusb_tegra.rs`), and
///   - the address NVIDIA's own UEFI `XhciControllerDxe` binds to on this SoC.
/// "Discovery" here means: this is the well-known fixed MMIO region for the XUSB/xHCI
/// host block on every Tegra234-based Orin module, not a value probed for at runtime.
const XUSB_HOST: u64 = 0x0361_0000;

/// xHCI operational-register offsets, relative to `op` (= `XUSB_HOST + CAPLENGTH`).
/// Matches the layout UnaOS's kernel driver reads at the same offsets
/// (`unaos/crates/kernel/src/drivers/xhci/mod.rs`, `unaos/crates/kernel/src/arch/aarch64/xusb_tegra.rs`).
const OP_USBCMD: u64 = 0x00;
const OP_USBSTS: u64 = 0x04;
const OP_CONFIG: u64 = 0x38;
const OP_CRCR: u64 = 0x18;

#[inline(always)]
unsafe fn r32(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline(always)]
unsafe fn r64(addr: u64) -> u64 {
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

#[inline(always)]
unsafe fn w64(addr: u64, v: u64) {
    unsafe { core::ptr::write_volatile(addr as *mut u64, v) }
}

fn banner() {
    info!("================================================================================");
    info!(" orin-xhci-repro -- UnaOS reproducer for NVIDIA forum topic 377113");
    info!(" xHCI-takeover RAS carveout fault on Tegra234 / Jetson Orin");
    info!(" Standalone UEFI app. No UnaOS kernel, no UnaOS build tooling required.");
    info!("================================================================================");
}

fn warning_banner() {
    warn!("################################################################################");
    warn!("# WARNING -- THE NEXT ACTION IS A FAULT-INDUCING REGISTER WRITE               #");
    warn!("#                                                                              #");
    warn!("# On an affected / \"poisoned\" boot, confirming below WILL power off this      #");
    warn!("# board via a hardware RAS event (Tegra234 SNOC/ACI carveout fault). THIS IS   #");
    warn!("# THE EXPECTED, INTENDED RESULT -- it is the reproduction, not a crash of      #");
    warn!("# this tool. Do not report it as a bug in this program.                        #");
    warn!("#                                                                              #");
    warn!("# On a \"clean\" boot (see the reply draft for observed per-boot odds), nothing  #");
    warn!("# visible happens and this program continues and exits normally.               #");
    warn!("#                                                                              #");
    warn!("# Only proceed on a board you intend to power-cycle right now. This is the     #");
    warn!("# ONLY register this program ever writes; everything above was reads only.     #");
    warn!("################################################################################");
}

/// Block for exactly one keystroke and return it. Mirrors the `uefi` crate's own
/// documented pattern (`Input::read_key` docs): arm the wait-for-key event, block on it via
/// `boot::wait_for_event`, then read the key that unblocked it.
fn read_one_key() -> Option<Key> {
    system::with_stdin(|stdin| {
        if let Ok(evt) = stdin.wait_for_key_event() {
            let mut events = [evt];
            let _ = boot::wait_for_event(&mut events);
        }
        stdin.read_key().unwrap_or(None)
    })
}

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    banner();

    // ---------------------------------------------------------------------------------
    // Discovery (read-only). See the XUSB_HOST doc comment: fixed platform constant, the
    // same one UnaOS's own takeover uses, not something probed for.
    // ---------------------------------------------------------------------------------
    info!(
        "XUSB/xHCI host MMIO base: {:#010x} (fixed Tegra234 platform constant -- the \
         `usb@3610000` device-tree node; not PCI-discovered)",
        XUSB_HOST
    );

    let cap0 = unsafe { r32(XUSB_HOST) };
    if cap0 == 0xFFFF_FFFF || cap0 == 0 {
        error!(
            "cap0={:#010x} at {:#010x} -- controller does not look alive/mapped here. STOP.",
            cap0, XUSB_HOST
        );
        boot::stall(Duration::from_secs(5));
        return Status::DEVICE_ERROR;
    }
    let caplen = (cap0 & 0xff) as u64;
    let hciversion = (cap0 >> 16) & 0xffff;
    let hcsparams1 = unsafe { r32(XUSB_HOST + 0x04) };
    let max_slots = hcsparams1 & 0xff;
    let max_ports = (hcsparams1 >> 24) & 0xff;
    info!(
        "cap0={:#010x} CAPLENGTH={:#04x} HCIVERSION={:#06x} HCSPARAMS1={:#010x} \
         (MaxSlots={} MaxPorts={})",
        cap0, caplen, hciversion, hcsparams1, max_slots, max_ports
    );

    let op = XUSB_HOST + caplen;
    let usbcmd_addr = op + OP_USBCMD;
    let usbsts_addr = op + OP_USBSTS;
    let config_addr = op + OP_CONFIG;
    let crcr_addr = op + OP_CRCR;

    let usbcmd = unsafe { r32(usbcmd_addr) };
    let usbsts = unsafe { r32(usbsts_addr) };
    let config = unsafe { r32(config_addr) };
    let crcr = unsafe { r64(crcr_addr) };

    let rs = usbcmd & 1;
    let hch = usbsts & 1;
    let cnr = (usbsts >> 11) & 1;
    let rcs = crcr & 1;
    let crr = (crcr >> 3) & 1;

    info!("---- operational registers (READ-ONLY so far) --------------------------------");
    info!("USBCMD  @ {:#010x} = {:#010x}   (RS={})", usbcmd_addr, usbcmd, rs);
    info!("USBSTS  @ {:#010x} = {:#010x}   (HCH={} CNR={})", usbsts_addr, usbsts, hch, cnr);
    info!("CONFIG  @ {:#010x} = {:#010x}   (MaxSlotsEn={})", config_addr, config, config & 0xff);
    info!(
        "CRCR    @ {:#010x} = {:#018x}   (RCS={} CRR={}) -- pointer field always reads \
         as zero per xHCI 5.4.5, regardless of CRR",
        crcr_addr, crcr, rcs, crr
    );
    info!("--------------------------------------------------------------------------------");

    if rs != 1 {
        warn!(
            "RS=0 -- this controller is NOT currently running. The reported fault's \
             precondition is a RUNNING (RS=1) UEFI-inherited controller; writing CRCR now \
             would not exercise the reported mechanism. Refusing the gated write."
        );
        warn!(
            "This is unexpected on stock firmware straight off a cold boot -- if you see \
             this, please report it exactly as-is (it may itself be informative): something \
             else touched this controller (RS=0/HCH=1) before this app ran."
        );
        boot::stall(Duration::from_secs(5));
        return Status::SUCCESS;
    }
    info!("RS=1 confirmed -- controller is UEFI-inherited and RUNNING (fault precondition met).");

    // ---------------------------------------------------------------------------------
    // Gated trigger. Nothing above this point wrote to the controller.
    // ---------------------------------------------------------------------------------
    warning_banner();
    info!("Press Y to issue the single triggering CRCR write now. Any other key aborts.");

    let key = read_one_key();
    let confirmed = matches!(
        key,
        Some(Key::Printable(c))
            if c == Char16::try_from('Y').unwrap() || c == Char16::try_from('y').unwrap()
    );

    if !confirmed {
        info!("Aborted by operator (no Y) -- no write issued. Exiting read-only.");
        boot::stall(Duration::from_secs(3));
        return Status::SUCCESS;
    }

    warn!("Operator confirmed -- issuing the single gated CRCR write in 1 second...");
    boot::stall(Duration::from_secs(1));

    // THE ONE MUTATION: a single 64-bit write to CRCR, matching the SHAPE of the validated
    // trigger write as closely as a standalone tool (no ring, no takeover) can.
    //
    // We deliberately do NOT echo the value read back. A CRCR read always returns the
    // pointer field as zero (xHCI 5.4.5); since RS=1 implies the ring was left stopped by
    // UEFI's own driver (CRR=0 is the expected/observed state here), an echo write would
    // LOAD that zero as a real Command Ring Pointer -- a NULL pointer is a genuine, if
    // inert, state change, and NOT the shape of the write UnaOS's own bench validated as
    // the trigger (which always carried a real, non-null, freshly-allocated ring pointer
    // with RCS=1 set). So instead this writes a SYNTHETIC pointer of that same shape: a
    // plausible, 64-byte-aligned, non-null address inside the Orin DRAM window
    // (2 GiB - 10 GiB PA) with RCS=1 (bit 0) set -- `synthetic_ring_ptr | 1`, mirroring the
    // kernel's own `our_ring | 1` construction. This program never rings a doorbell, so the
    // controller has no occasion to dereference this address; only the register write
    // itself -- a CRCR write while RS=1 -- is exercised, which is the trigger under test.
    const SYNTHETIC_RING_PTR: u64 = 0x2_0000_0000; // 8 GiB PA: inside the DRAM window, 64-byte aligned, never dereferenced (no doorbell rung)
    let crcr_write_val = SYNTHETIC_RING_PTR | 1; // RCS=1
    unsafe { w64(crcr_addr, crcr_write_val) };

    warn!(
        "CRCR write issued: {:#018x} written to {:#010x} (synthetic non-null pointer, RCS=1 -- never dereferenced).",
        crcr_write_val, crcr_addr
    );
    warn!(
        "If this boot is poisoned, a RAS power-off should already be in flight. If you are \
         reading this line, this boot did not fault -- see the reply draft for the observed \
         per-boot clean/poisoned rate (this is a real coin flip, not a tool failure)."
    );

    let crcr_after = unsafe { r64(crcr_addr) };
    let usbsts_after = unsafe { r32(usbsts_addr) };
    info!(
        "Post-write: CRCR={:#018x} USBSTS={:#010x} (HCH={})",
        crcr_after,
        usbsts_after,
        usbsts_after & 1
    );
    info!("No further writes will be issued. Reboot to try another boot's coin flip.");

    boot::stall(Duration::from_secs(10));
    Status::SUCCESS
}
