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

    println!("🔹 Locating OVMF (x86_64 UEFI Firmware)...");
    let ovmf_paths = [
        "/usr/share/OVMF/OVMF_CODE.fd",
        "/usr/share/edk2/ovmf/OVMF_CODE.fd",
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd",
        "/usr/share/qemu/OVMF.fd",
    ];

    let mut ovmf_path = None;
    for path in &ovmf_paths {
        if std::path::Path::new(path).exists() {
            ovmf_path = Some(*path);
            break;
        }
    }

    let ovmf_path = ovmf_path.expect("CRITICAL ERROR: OVMF Firmware not found.");

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
    
    cmd.arg("-drive").arg(format!("if=pflash,format=raw,readonly=on,file={}", ovmf_path))
       .arg("-drive").arg(format!("format=raw,file=fat:rw:{}", esp_dir.display()))
       .arg("-serial").arg("stdio")
       .arg("-device").arg("isa-debug-exit,iobase=0xf4,iosize=0x04")
       .arg("-device").arg("qemu-xhci,id=xhci")
       .arg("-drive").arg(format!("if=none,id=stick,format=raw,file={}", usb_image.display()))
       .arg("-device").arg("usb-storage,bus=xhci.0,drive=stick")
       .arg("-device").arg("usb-tablet,bus=xhci.0")
       .arg("-m").arg("1G");

    let mut child = cmd.spawn().unwrap();
    child.wait().unwrap();
}
