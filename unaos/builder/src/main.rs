// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn main() {
    let workspace_dir = std::fs::canonicalize("..").unwrap();
    let target_dir = workspace_dir.join("target");
    let esp_dir = target_dir.join("x86_64_esp");

    println!("🔹 Building x86_64 Kernel...");
    let kernel_status = Command::new("cargo")
        .current_dir(workspace_dir.join("crates/kernel"))
        .arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--target").arg("../../x86_64-unaos.json")
        .arg("-Z").arg("build-std=core,compiler_builtins,alloc")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem")
        .arg("-Z").arg("json-target-spec")
        .status()
        .unwrap();

    if !kernel_status.success() {
        panic!("Kernel build failed");
    }

    println!("🔹 Building x86_64 UEFI Bootloader...");
    let bootloader_status = Command::new("cargo")
        .current_dir(workspace_dir.join("crates/bootloader"))
        .arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--target").arg("x86_64-unknown-uefi")
        .arg("-Z").arg("build-std=core,compiler_builtins,alloc")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem")
        .status()
        .unwrap();

    if !bootloader_status.success() {
        panic!("Bootloader build failed");
    }

    let kernel_bin = target_dir.join("x86_64-unaos/release/unaos-kernel");
    let bootloader_bin = target_dir.join("x86_64-unknown-uefi/release/bootloader.efi");

    println!("🔹 Packaging ESP (EFI System Partition)...");
    let _ = std::fs::remove_dir_all(&esp_dir);
    let boot_dir = esp_dir.join("EFI/BOOT");
    std::fs::create_dir_all(&boot_dir).unwrap();
    
    std::fs::copy(&bootloader_bin, boot_dir.join("BOOTX64.EFI")).unwrap();
    std::fs::copy(&kernel_bin, esp_dir.join("kernel.elf")).unwrap();

    // Package-only mode: build + pack the ESP, then stop (no QEMU). Used to produce real-hardware
    // boot media — copy this directory's contents onto a FAT32 USB and boot the Mac via Option.
    if std::env::var("UNAOS_PACKAGE_ONLY").is_ok() {
        println!(
            "✅ x86_64 ESP packaged at {} (EFI/BOOT/BOOTX64.EFI + kernel.elf)",
            esp_dir.display()
        );
        return;
    }

    println!("🔹 Locating OVMF (x86_64 UEFI Firmware)...");
    // Search is additive across platforms: macOS/Homebrew first (where this may run
    // for fast iteration), then the original Linux locations (unchanged behavior).
    let ovmf_code_paths = [
        "/usr/local/share/qemu/edk2-x86_64-code.fd",   // macOS Homebrew (Intel)
        "/opt/homebrew/share/qemu/edk2-x86_64-code.fd", // macOS Homebrew (Apple Silicon)
        "/usr/share/OVMF/OVMF_CODE.fd",
        "/usr/share/edk2/ovmf/OVMF_CODE.fd",
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd",
        "/usr/share/qemu/OVMF.fd",
    ];
    // Matching writable variable store. Split-firmware setups (macOS Homebrew, modern
    // Linux) need this as a second pflash unit; if none is found we fall back to a
    // single read-only code pflash (the original behavior).
    let ovmf_vars_paths = [
        "/usr/local/share/qemu/edk2-i386-vars.fd",
        "/opt/homebrew/share/qemu/edk2-i386-vars.fd",
        "/usr/share/OVMF/OVMF_VARS.fd",
        "/usr/share/edk2/ovmf/OVMF_VARS.fd",
        "/usr/share/edk2-ovmf/x64/OVMF_VARS.fd",
    ];

    let ovmf_code = ovmf_code_paths.iter().find(|p| std::path::Path::new(p).exists())
        .copied().expect("CRITICAL ERROR: OVMF code firmware not found.");

    // Copy the vars template to a writable per-run location (never write the template).
    let ovmf_vars = ovmf_vars_paths.iter().find(|p| std::path::Path::new(p).exists()).copied();
    let vars_writable = ovmf_vars.map(|template| {
        let dst = target_dir.join("OVMF_VARS.fd");
        std::fs::copy(template, &dst).expect("Failed to copy OVMF vars template");
        dst
    });
    println!("   code: {}  vars: {}", ovmf_code,
        vars_writable.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<none>".into()));

    // UNA-22-HAUL: Create a phantom drive (64MB)
    let usb_image = workspace_dir.join("builder/usb.img");
    if !usb_image.exists() {
        let mut file = std::fs::File::create(&usb_image).unwrap();
        file.set_len(64 * 1024 * 1024).unwrap(); // 64MB Sparse File

        // UNA-22-MANIFEST: Inject Signature
        use std::io::Write;
        file.write_all(b"UNA-OS-DISK-001-ALPHA").unwrap();

        println!("Created usb.img (64MB) with Signature.");
    }

    println!("🔹 Launching QEMU...");
    let mut cmd = Command::new("qemu-system-x86_64");

    // Q35/ICH9 chipset: PCIe-based, closer to real modern hardware (e.g. a 2012 MacBook
    // Pro) than the legacy i440FX default. Note: on Q35 the qemu-xhci PCIe INTx routes to
    // an APIC GSI the 8259 PIC cannot service, so interrupt-driven xHCI uses MSI-X (local
    // APIC) rather than legacy INTx.
    cmd.arg("-machine").arg("pc-q35-10.0");

    // Firmware: read-only code pflash (unit 0) + writable vars pflash (unit 1) when a
    // vars store exists; otherwise a single read-only code pflash (legacy behavior).
    cmd.arg("-drive").arg(format!("if=pflash,unit=0,format=raw,readonly=on,file={}", ovmf_code));
    if let Some(ref vars) = vars_writable {
        cmd.arg("-drive").arg(format!("if=pflash,unit=1,format=raw,file={}", vars.display()));
    }

    cmd.arg("-drive").arg(format!("format=raw,file=fat:rw:{}", esp_dir.display()))
       .arg("-device").arg("isa-debug-exit,iobase=0xf4,iosize=0x04")
       .arg("-device").arg("qemu-xhci,id=xhci")
       .arg("-drive").arg(format!("if=none,id=stick,format=raw,file={}", usb_image.display()))
       .arg("-device").arg("usb-storage,bus=xhci.0,drive=stick")
       .arg("-device").arg("usb-kbd,bus=xhci.0")
       .arg("-device").arg("usb-tablet,bus=xhci.0")
       .arg("-m").arg("1G");

    // DIAGNOSTIC: append arbitrary QEMU args from UNAOS_QEMU_EXTRA (whitespace-split), e.g.
    // `-d guest_errors -trace usb_xhci_* -trace usb_msd_*`, so we can capture QEMU's own
    // tracing of the xHCI/SCSI path. In test mode QEMU's stderr is redirected to a file.
    let qemu_extra = std::env::var("UNAOS_QEMU_EXTRA").unwrap_or_default();
    for a in qemu_extra.split_whitespace() {
        cmd.arg(a);
    }

    // Test mode: set UNAOS_SERIAL_LOG to run headless, redirect serial to that file, and
    // self-terminate after UNAOS_TEST_SECS (default 20s). Keeps automated boot-log capture
    // portable (no `timeout` binary needed). Normal runs keep the GUI + serial on stdio.
    if let Ok(log_path) = std::env::var("UNAOS_SERIAL_LOG") {
        let secs: u64 = std::env::var("UNAOS_TEST_SECS").ok()
            .and_then(|s| s.parse().ok()).unwrap_or(20);
        println!("   [test mode] headless, serial -> {log_path}, auto-kill after {secs}s");
        cmd.arg("-display").arg("none")
           .arg("-serial").arg(format!("file:{log_path}"));
        // Capture QEMU's own stderr (where -d / -trace output goes) next to the serial log.
        if let Ok(dbg_path) = std::env::var("UNAOS_QEMU_DEBUG_LOG") {
            if let Ok(f) = std::fs::File::create(&dbg_path) {
                cmd.stderr(Stdio::from(f));
            }
        }
        let mut child = cmd.spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(secs));
        let _ = child.kill();
        let _ = child.wait();
    } else {
        cmd.arg("-serial").arg("stdio");
        let mut child = cmd.spawn().unwrap();
        child.wait().unwrap();
    }
}
