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

use std::process::{Command, Stdio};

fn main() {
    let workspace_dir = std::fs::canonicalize("..").unwrap();
    let target_dir = workspace_dir.join("target");
    let esp_dir = target_dir.join("x86_64_esp");

    println!("🔹 Building x86_64 Kernel...");
    let mut kernel_cmd = Command::new("cargo");
    kernel_cmd
        .current_dir(workspace_dir.join("crates/kernel"))
        .arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--target").arg("../../x86_64-unaos.json")
        .arg("-Z").arg("build-std=core,compiler_builtins,alloc")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem")
        .arg("-Z").arg("json-target-spec");
    // Optional kernel features from env knobs: UNAOS_SKIP_XHCI=1 (disable xHCI/USB bring-up),
    // UNAOS_BOOTLOG=1 (hold the boot log on screen instead of the GUI), UNAOS_USBDEBUG=1 (run the
    // USB main loop but keep the boot log on screen + print input events). Composable. NOTE: keep
    // this list in sync with arroyo's feature mapping — the builder rebuilds the kernel, so a knob
    // missing here is silently dropped even if arroyo set it.
    let mut feats: Vec<&str> = Vec::new();
    if std::env::var("UNAOS_SKIP_XHCI").is_ok() { feats.push("skip_xhci"); }
    if std::env::var("UNAOS_BOOTLOG").is_ok() { feats.push("bootlog"); }
    if std::env::var("UNAOS_PI").is_ok() { feats.push("pi"); }
    if std::env::var("UNAOS_USBDEBUG").is_ok() { feats.push("usbdebug"); }
    if std::env::var("UNAOS_SCHED_DEMO").is_ok() { feats.push("sched_demo"); }
    if !feats.is_empty() {
        let list = feats.join(",");
        kernel_cmd.arg("--features").arg(&list);
        println!("   kernel features: {list}");
    }
    let kernel_status = kernel_cmd.status().unwrap();

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

    // Network backend selector. Default is user-mode (slirp): zero privileges, link
    // comes up, but the host cannot arping/ping the guest (it's NAT). UNAOS_NET=vmnet
    // switches to a vmnet-host netdev so host and guest share an L2 segment — enabling
    // `ping`/`arping 10.0.2.15` from the host. vmnet needs root, so QEMU runs under sudo.
    let net_mode = std::env::var("UNAOS_NET").unwrap_or_default();
    let use_vmnet = net_mode == "vmnet";

    println!("🔹 Launching QEMU{}...", if use_vmnet { " (vmnet-host, via sudo)" } else { "" });
    let mut cmd = if use_vmnet {
        let mut c = Command::new("sudo");
        c.arg("qemu-system-x86_64");
        c
    } else {
        Command::new("qemu-system-x86_64")
    };

    // Q35/ICH9 chipset: PCIe-based, closer to real modern hardware (e.g. a 2012 MacBook
    // Pro) than the legacy i440FX default. Note: on Q35 the qemu-xhci PCIe INTx routes to
    // an APIC GSI the 8259 PIC cannot service, so interrupt-driven xHCI uses MSI-X (local
    // APIC) rather than legacy INTx.
    cmd.arg("-machine").arg("pc-q35-10.0");

    // CPU model: advertise x2APIC (the default qemu64 model does not), so the kernel exercises
    // the MSR-based local-APIC path that the target hardware (2012 MacBook, Zenbook S16) uses.
    // Override with UNAOS_CPU — e.g. `UNAOS_CPU=qemu64` drops x2APIC to test the xAPIC fallback.
    let cpu = std::env::var("UNAOS_CPU").unwrap_or_else(|_| "qemu64,+x2apic".into());
    cmd.arg("-cpu").arg(cpu);

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

    // SMP: bring up multiple CPUs so the kernel's AP-startup path has application
    // processors to discover (ACPI MADT) and boot (INIT-SIPI-SIPI). Override the core
    // count with UNAOS_SMP (e.g. `UNAOS_SMP=1` to force uniprocessor). The BSP still
    // drives xHCI/console/storage; APs idle until the scheduler work lands.
    let smp = std::env::var("UNAOS_SMP").unwrap_or_else(|_| "4".into());
    cmd.arg("-smp").arg(smp);

    // Network: Intel e1000 (82540EM). slirp (default) brings the link up but the host
    // can't reach the guest (NAT). vmnet-host shares an L2 segment on 10.0.2.0/24
    // (host = 10.0.2.1) so `ping`/`arping` of the guest's static 10.0.2.15 works.
    // For slirp wire debugging add UNAOS_QEMU_EXTRA="-object filter-dump,id=d0,netdev=n0,file=target/net.pcap".
    if use_vmnet {
        cmd.arg("-netdev")
            .arg("vmnet-host,id=n0,start-address=10.0.2.1,end-address=10.0.2.254,subnet-mask=255.255.255.0");
    } else if net_mode == "socket" {
        // Rootless L2 link to a host injector (scripts/net-inject.py) for automated
        // ARP/ICMP responder testing without privileges. QEMU listens; injector connects.
        cmd.arg("-netdev").arg("socket,id=n0,listen=127.0.0.1:5555");
    } else {
        // dhcpstart shifts slirp's DHCP pool so a successful lease (10.0.2.20) is visibly
        // distinct from the guest's static fallback (10.0.2.15) — makes DHCP easy to confirm.
        cmd.arg("-netdev").arg("user,id=n0,dhcpstart=10.0.2.20");
    }
    cmd.arg("-device").arg("e1000e,netdev=n0,mac=52:54:00:12:34:56");

    // DIAGNOSTIC: append arbitrary QEMU args from UNAOS_QEMU_EXTRA (whitespace-split), e.g.
    // `-d guest_errors -trace usb_xhci_* -trace usb_msd_*`, so we can capture QEMU's own
    // tracing of the xHCI/SCSI path. In test mode QEMU's stderr is redirected to a file.
    let qemu_extra = std::env::var("UNAOS_QEMU_EXTRA").unwrap_or_default();
    for a in qemu_extra.split_whitespace() {
        cmd.arg(a);
    }

    // vmnet runs under sudo; signal-based self-kill does not propagate through sudo, so
    // run interactively (GUI) with serial -> file and a wire pcap, and let the user quit
    // QEMU (Ctrl-C in this terminal / close the window) when done testing.
    if use_vmnet {
        let log_path = std::env::var("UNAOS_SERIAL_LOG")
            .unwrap_or_else(|_| target_dir.join("serial.log").display().to_string());
        let pcap = target_dir.join("net.pcap");
        // Headless (GUI-under-sudo is unreliable on macOS); serial + wire pcap to files.
        // Stop with Ctrl-C in this terminal (SIGINT reaches QEMU via the process group).
        cmd.arg("-display").arg("none")
            .arg("-serial").arg(format!("file:{log_path}"))
            .arg("-object")
            .arg(format!("filter-dump,id=d0,netdev=n0,file={}", pcap.display()));
        println!("   [vmnet] headless; serial -> {log_path}");
        println!("   [vmnet] wire pcap -> {}", pcap.display());
        println!("   [vmnet] guest IP 10.0.2.15. In another terminal, find the host vmnet iface IP:");
        println!("   [vmnet]   ifconfig | grep -B3 10.0.2     (expect a bridge/vmnet iface at 10.0.2.1)");
        println!("   [vmnet] then:  ping -c3 10.0.2.15        (ping does ARP first, so it tests ARP + ICMP)");
        println!("   [vmnet] watch replies in {log_path}  and  tcpdump -r {} -nne", pcap.display());
        println!("   [vmnet] Ctrl-C here to stop QEMU when done.");
        let mut child = cmd.spawn().unwrap();
        child.wait().unwrap();
        return;
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
