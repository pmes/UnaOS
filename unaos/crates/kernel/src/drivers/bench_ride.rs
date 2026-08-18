//! BENCH-RIDE — read-only evidence probes that ride the rMBP kepler sitting boots.
//!
//! Every kepler/display sitting costs an attended reboot; these probes harvest ground truth
//! from the same boots for free. Each is knob-gated, one-shot, prints a single grep-able
//! `:: <tag>: ... ::` line block on serial, and performs ZERO device writes — safe to ride
//! any sitting without perturbing the GPU lanes. They run after xHCI (serial is live) and
//! BEFORE the GPU dispatch, so their evidence survives a GPU-init wedge.
//!
//! - `thermprobe` (`UNAOS_THERM=1`, implies `smc`): SMC thermal + fan snapshot — CPU/GPU
//!   proximity and die temps (SP78 keys) and actual fan RPM (fpe2). Ground truth for how hot
//!   the GK107 runs across takeover boots.
//! - `pcilink` (`UNAOS_PCILINK=1`): per-display-device PCIe link state — PM D-state (PMCSR)
//!   and negotiated link gen/width vs capability (LnkSta/LnkCap). Tells us what power/link
//!   regime the firmware leaves the dGPU in before we ever touch it.
//! - `vromprobe` (`UNAOS_VROM=1`): dGPU expansion-ROM sniff — reports the ROM BAR as the
//!   firmware left it and, ONLY if already assigned+enabled, reads the 0x55AA/PCIR header
//!   (image count/size/code type). Seeds the K-GPU-4 Falcon-ucode work (VBIOS init tables)
//!   with facts. Never enables ROM decode itself: unassigned/disabled = report and stop.

#![allow(dead_code)]

/// SMC thermal + fan snapshot. SP78 temperature keys print the signed integer part in °C;
/// fpe2 fan keys print RPM. Missing keys are skipped silently (key sets vary per SMC rev);
/// an absent/unresponsive SMC prints the honest no-smc line.
#[cfg(feature = "thermprobe")]
pub fn therm_snapshot() {
    use crate::drivers::smc;
    if !smc::present() {
        serial_println!(":: therm: no-smc ::");
        return;
    }
    // (key, label): proximity + die sensors for CPU and GPU, plus heatpipe/palm rest.
    const TEMPS: &[(&[u8; 4], &str)] = &[
        (b"TC0P", "cpu-prox"),
        (b"TC0D", "cpu-die"),
        (b"TG0P", "gpu-prox"),
        (b"TG0D", "gpu-die"),
        (b"Th1H", "heatpipe"),
        (b"Ts0P", "palm"),
    ];
    let mut printed = false;
    for (key, label) in TEMPS {
        let mut buf = [0u8; 2];
        if let Ok(n) = smc::read_key(key, &mut buf) {
            if n >= 1 {
                // SP78: signed fixed-point, integer °C in byte 0. 0 with 0 fraction on a
                // real sensor is implausible-but-possible; print whatever the SMC said.
                serial_println!(":: therm: {}={}C ::", label, buf[0] as i8);
                printed = true;
            }
        }
    }
    const FANS: &[(&[u8; 4], &str)] = &[(b"F0Ac", "fan0"), (b"F1Ac", "fan1")];
    for (key, label) in FANS {
        let mut buf = [0u8; 2];
        if let Ok(n) = smc::read_key(key, &mut buf) {
            if n >= 2 {
                // fpe2: unsigned fixed-point, value = u16 >> 2.
                let raw = ((buf[0] as u16) << 8) | buf[1] as u16;
                serial_println!(":: therm: {}={}rpm ::", label, raw >> 2);
                printed = true;
            }
        }
    }
    if !printed {
        serial_println!(":: therm: smc-present-no-keys ::");
    }
}

/// PCIe link/power snapshot for every display-class (0x03) function: PM D-state from PMCSR
/// and negotiated vs maximum link speed/width from the PCIe capability. Config reads only.
#[cfg(feature = "pcilink")]
pub fn pcilink_snapshot() {
    for bus in 0u8..=8 {
        for slot in 0u8..32 {
            for func in 0u8..8 {
                let vendor = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x00) };
                if vendor == 0xFFFF {
                    if func == 0 { break; } else { continue; }
                }
                let class_reg = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x08) };
                if ((class_reg >> 24) & 0xFF) as u8 != 0x03 {
                    continue;
                }
                let device = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x02) };
                let status = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x06) };
                if status & (1 << 4) == 0 {
                    serial_println!(":: pcilink: {:04x}:{:04x} at {}:{}.{} no-caplist ::",
                        vendor, device, bus, slot, func);
                    continue;
                }
                let mut dstate: i8 = -1;
                let mut lnk: Option<(u16, u32)> = None; // (LnkSta, LnkCap)
                let mut ptr = (unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x34) } & 0xFC) as u8;
                let mut hops = 0;
                while ptr != 0 && hops < 48 {
                    let hdr = unsafe { crate::arch::pci::read_config_16(bus, slot, func, ptr) };
                    match (hdr & 0xFF) as u8 {
                        0x01 => {
                            let pmcsr = unsafe { crate::arch::pci::read_config_16(bus, slot, func, ptr + 4) };
                            dstate = (pmcsr & 0x3) as i8;
                        }
                        0x10 => {
                            let cap = unsafe { crate::arch::pci::read_config_32(bus, slot, func, ptr + 0x0C) };
                            let sta = unsafe { crate::arch::pci::read_config_16(bus, slot, func, ptr + 0x12) };
                            lnk = Some((sta, cap));
                        }
                        _ => {}
                    }
                    ptr = ((hdr >> 8) & 0xFC) as u8;
                    hops += 1;
                }
                match lnk {
                    Some((sta, cap)) => serial_println!(
                        ":: pcilink: {:04x}:{:04x} at {}:{}.{} D{} gen{}x{} (cap gen{}x{}) ::",
                        vendor, device, bus, slot, func, dstate,
                        sta & 0xF, (sta >> 4) & 0x3F,
                        cap & 0xF, (cap >> 4) & 0x3F),
                    None => serial_println!(
                        ":: pcilink: {:04x}:{:04x} at {}:{}.{} D{} no-pcie-cap ::",
                        vendor, device, bus, slot, func, dstate),
                }
            }
        }
    }
    serial_println!(":: pcilink: scan-done ::");
}

/// dGPU expansion-ROM sniff. Reports the ROM BAR exactly as the firmware left it; only if
/// the BAR is already assigned AND decode-enabled does it map the window and read the
/// 0x55AA / PCIR header chain. This probe performs no config or MMIO writes — an
/// unassigned or disabled ROM is reported honestly and left alone.
#[cfg(feature = "vromprobe")]
pub fn vrom_sniff() {
    // NVIDIA display-class functions only.
    for bus in 0u8..=8 {
        for slot in 0u8..32 {
            let vendor = unsafe { crate::arch::pci::read_config_16(bus, slot, 0, 0x00) };
            if vendor != 0x10DE {
                continue;
            }
            let class_reg = unsafe { crate::arch::pci::read_config_32(bus, slot, 0, 0x08) };
            if ((class_reg >> 24) & 0xFF) as u8 != 0x03 {
                continue;
            }
            let rombar = unsafe { crate::arch::pci::read_config_32(bus, slot, 0, 0x30) };
            let addr = (rombar & 0xFFFF_F800) as u64;
            let enabled = rombar & 1 != 0;
            serial_println!(":: vrom: {}:{}.0 rombar={:#010x} addr={:#x} en={} ::",
                bus, slot, rombar, addr, enabled as u8);
            if !enabled || addr == 0 {
                serial_println!(":: vrom: rom-not-decoded (read-only policy: leaving it) ::");
                continue;
            }
            crate::arch::memory::map_mmio_window(addr, 0x1_0000);
            if crate::arch::memory::translate(addr).is_none() {
                serial_println!(":: vrom: rom-window-unmapped ::");
                continue;
            }
            let r8 = |off: u64| unsafe { ((addr + off) as *const u8).read_volatile() };
            let r16 = |off: u64| unsafe { ((addr + off) as *const u16).read_volatile() };
            let sig = r16(0);
            if sig != 0xAA55 {
                serial_println!(":: vrom: bad-rom-sig {:#06x} ::", sig);
                continue;
            }
            let pcir_off = r16(0x18) as u64;
            let pcir = [r8(pcir_off), r8(pcir_off + 1), r8(pcir_off + 2), r8(pcir_off + 3)];
            if &pcir != b"PCIR" {
                serial_println!(":: vrom: sig-ok pcir-missing at {:#x} ::", pcir_off);
                continue;
            }
            let img_len = r16(pcir_off + 0x10) as u64 * 512;
            let code_type = r8(pcir_off + 0x14);
            let indicator = r8(pcir_off + 0x15);
            serial_println!(
                ":: vrom: sig-ok pcir-ok ven={:04x} dev={:04x} img={}B code-type={:#04x} last={} ::",
                r16(pcir_off + 4), r16(pcir_off + 6), img_len, code_type, (indicator >> 7) & 1);
        }
    }
    serial_println!(":: vrom: scan-done ::");
}
