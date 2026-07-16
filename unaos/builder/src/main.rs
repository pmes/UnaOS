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
    // USB main loop but keep the boot log on screen + print input events), UNAOS_PI=1 (Pi 4,
    // fbcon-only serial) and UNAOS_TEGRA=1 (Jetson Orin / Tegra234 UART). Composable. NOTE: keep
    // this list in sync with arroyo's feature mapping — the builder rebuilds the kernel, so a knob
    // missing here is silently dropped even if arroyo set it.
    let mut feats: Vec<&str> = Vec::new();
    if std::env::var("UNAOS_SKIP_XHCI").is_ok() { feats.push("skip_xhci"); }
    if std::env::var("UNAOS_BOOTLOG").is_ok() { feats.push("bootlog"); }
    if std::env::var("UNAOS_PI").is_ok() { feats.push("pi"); }
    if std::env::var("UNAOS_USBDEBUG").is_ok() { feats.push("usbdebug"); }
    if std::env::var("UNAOS_SCHED_DEMO").is_ok() { feats.push("sched_demo"); }
    // UNAOS_IRQSTORAGE=1 routes x86 storage syscalls through the interrupt-driven storage service task
    // (STOR-1) instead of the staged-buffer path. x86_64 only; a no-op on the aarch64 media the arroyo
    // script builds. Metal-pending, so it stays opt-in.
    if std::env::var("UNAOS_IRQSTORAGE").is_ok() { feats.push("irqstorage"); }
    // VPERF: x86 video-path bench instrumentation (scroll/VRAM-read counters, fbmem readout,
    // display-BAR probe, scripted scroll scenario). x86_64-only module; default OFF.
    if std::env::var("UNAOS_VIDEOBENCH").is_ok() { feats.push("videobench"); }
    // PORTSW-1: the Panther Point EHCI->xHCI port switchover runs BY DEFAULT (metal-gated policy
    // 2026-07-16: the no-routing boot dropped ALL external USB on the 2012 rMBP). UNAOS_NOPORTSW=1
    // OPTS OUT (never-run no-routing experiment) => zero config-space writes, byte-identical no-routing
    // media; inert on QEMU (non-Intel xHCI). x86_64 only. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_NOPORTSW").is_ok() { feats.push("noportsw"); }
    // EHCI-1 scout: UNAOS_EHCISCOUT=1 arms a STRICTLY READ-ONLY EHCI reconnaissance probe (dumps the
    // EHCI companion controllers' cap/op/PORTSC state at boot; zero writes). x86_64-only module.
    // Kept in sync with arroyo's mapping; also adds a QEMU `-device usb-ehci` test target below.
    if std::env::var("UNAOS_EHCISCOUT").is_ok() { feats.push("ehciscout"); }
    // VPERF M2: the fbcon viewport-cap bench lever (implies videobench). x86_64 only.
    if std::env::var("UNAOS_VIDEOCAP").is_ok() { feats.push("videocap"); }
    // SOCK-1: route the shell's ping/arp/netinfo + the boot ICMP witness through smoltcp. x86-only
    // optional dep + module. (The builder rebuilds the kernel, so this MUST mirror arroyo's mapping.)
    if std::env::var("UNAOS_SMOLNET").is_ok() { feats.push("smolnet"); }
    // `tegra` (Jetson Orin / Tegra234 UART) is an aarch64 board feature; mapped here for parity with
    // the `pi` knob, though this x86_64 builder never produces aarch64 media (the `arroyo` script does).
    if std::env::var("UNAOS_TEGRA").is_ok() { feats.push("tegra"); }
    // ORIN-SMP-2: UNAOS_SMPPROBE=<n> arms the JM5 CPU_ON firmware-wall investigation probe (tegra-only
    // aarch64; the numeric value selects the experiment via option_env). Mapped here for parity with
    // arroyo's feature list, though this x86_64 builder never produces the aarch64 tegra media.
    if std::env::var("UNAOS_SMPPROBE").is_ok() { feats.push("smpprobe"); }
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

    // A small text file so the in-kernel FAT reader has something to `cat` on the real boot stick:
    // after boot, `ls` shows EFI/ + kernel.elf + hello.txt and `cat hello.txt` reads it back off the
    // FAT32 volume — proving USB mass-storage block I/O + FAT parsing on metal.
    std::fs::write(
        esp_dir.join("hello.txt"),
        "Hello from UnaOS on real hardware!\nThis file was read off the FAT32 boot stick by the in-kernel FAT reader.\n",
    ).unwrap();

    // U2: the x86 ring-3 "hello from disk" program (crates/user-blob-x86, built by arroyo's
    // build_user_hello_x86 to target/hello.bin). Copy it onto the ESP as HELLO.BIN so the metal boot
    // media carries it; the kernel's U2 FAT loader reads it off the volume and runs it in ring 3.
    // (make-fat-img.sh copies the same target/hello.bin onto the QEMU FAT stick images.) Absent when
    // the blob wasn't built (a bare `cargo run` in builder/) — then U2 simply NoFile-skips, harmless.
    let hello_bin = target_dir.join("hello.bin");
    if hello_bin.exists() {
        std::fs::copy(&hello_bin, esp_dir.join("HELLO.BIN")).unwrap();
        println!("   U2: copied HELLO.BIN onto the ESP");
    } else {
        println!("   U2: target/hello.bin absent — ESP has no HELLO.BIN (run via ./arroyo esp-x86)");
    }

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

    // UNAOS_FATIMG selects a FAT filesystem image (built by scripts/make-fat-img.sh) as the
    // usb-storage backing instead of the raw UNA-OS pattern image, giving the kernel's read-only
    // FAT reader (`ls`/`cat`) a real FAT32 volume to parse. `1`/`part` -> builder/fat.img,
    // `sf` -> builder/fat-sf.img, or an explicit path. Unset (the default) keeps usb.img so the
    // BOT "MISSION SUCCESS" pattern test is unchanged. block::info() registers this single device,
    // mirroring a real single-stick metal boot where the FAT32 ESP stick *is* the block device.
    let stick_image = match std::env::var("UNAOS_FATIMG").ok().as_deref() {
        None | Some("") => usb_image.clone(),
        Some("1") | Some("part") => workspace_dir.join("builder/fat.img"),
        Some("gpt") => workspace_dir.join("builder/fat-gpt.img"),
        Some("p16") => workspace_dir.join("builder/fat16.img"),
        Some("sf") => workspace_dir.join("builder/fat-sf.img"),
        Some(path) => std::path::PathBuf::from(path),
    };
    if stick_image != usb_image && !stick_image.exists() {
        panic!("UNAOS_FATIMG set but {} is missing — run `./arroyo fat-img` first",
            stick_image.display());
    }
    println!("   usb-storage backing: {}", stick_image.display());

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

    // Boot the ESP first, explicitly (bootindex=0). When UNAOS_FATIMG points the usb-storage at a
    // FAT image (which also carries an EFI/ tree), OVMF would otherwise sometimes *attempt* the USB
    // drive before the ESP; that boot-time USB touch destabilizes the kernel's later BOT reads of
    // the same device (flaky FAT mounts). Pinning the ESP to bootindex=0 (and the stick to a lower
    // priority) makes OVMF go straight to the ESP, leaving the usb-storage pristine for the kernel —
    // the raw usb.img default already behaved this way (OVMF never boots a non-FAT device).
    // qemu-xhci defaults to p2=4 USB2 ports. With the FTDI attached (U2.5) there are four full/high-
    // speed USB2 devices — storage + kbd + tablet + serial — and QEMU overflows the 4th onto an
    // AUTO-INSERTED usb-hub, putting the FTDI behind a hub. Hub-downstream enumeration handles
    // HID + mass storage but not FTDI, so a hubbed FTDI is never configured. Widen the root-port
    // count so every device lands
    // on a root port — but ONLY when the FTDI is attached, so the default (no-knob) runs keep the
    // exact 4/4 controller shape and a byte-identical boot log.
    let usbserial = std::env::var("UNAOS_USBSERIAL").is_ok();
    let xhci_dev = if usbserial { "qemu-xhci,id=xhci,p2=8,p3=8" } else { "qemu-xhci,id=xhci" };
    // UNAOS_NOSTORAGE=1 omits the usb-storage device entirely, so the kernel enumerates NO block device
    // (block::info() -> None) — the QEMU analog of the metal 2012 rMBP, where the SD reader never enumerates
    // over xHCI (the storage-enumeration blocker). Used to exercise the no-storage control path: the
    // storage-INDEPENDENT capability demos (U5x/U7x/U8x — inline console-cap blobs) run + print there, while
    // every storage-GATED arc (U2/U4x/U6x/U6bx/U9x/U11x) skips. Previews exactly what the metal FTDI console
    // shows. NOTE: `is_ok()` treats an EMPTY value as SET (the known knob trap) — `UNAOS_NOSTORAGE=` is ON.
    let nostorage = std::env::var("UNAOS_NOSTORAGE").is_ok();
    // UNAOS_HUBSTORAGE=1 attaches the usb-storage BEHIND a usb-hub instead of on a root port —
    // the QEMU reproduction of the metal rMBP failure mode where the SD reader sits downstream of
    // a hub and used to be left `class=0x0`/unconfigured (hub-downstream enumeration was HID-only).
    // Exercises the hub-downstream mass-storage path end to end (interface-level MSC detect +
    // Configure-Endpoint + BOT). NOTE: `is_ok()` — an EMPTY value is ON, like the other knobs.
    let hubstorage = std::env::var("UNAOS_HUBSTORAGE").is_ok();
    cmd.arg("-drive").arg(format!("if=none,id=esp,format=raw,file=fat:rw:{}", esp_dir.display()))
       .arg("-device").arg("ide-hd,drive=esp,bootindex=0")
       .arg("-device").arg("isa-debug-exit,iobase=0xf4,iosize=0x04")
       .arg("-device").arg(xhci_dev);
    // EHCI-1 scout (UNAOS_EHCISCOUT=1): give the read-only EHCI probe a QEMU target. q35's default
    // device set has no EHCI, so attach a standalone `usb-ehci` PCI controller (class 0x0C0320) — no
    // downstream device, so the scout reports the controller's cap/op/PORTSC state with 0 connected
    // ports (the honest QEMU result). This is a QEMU-harness knob, not a kernel write path.
    if std::env::var("UNAOS_EHCISCOUT").is_ok() {
        cmd.arg("-device").arg("usb-ehci,id=ehci");
        println!("   UNAOS_EHCISCOUT: usb-ehci controller attached — read-only EHCI scout target");
    }
    if nostorage {
        println!("   UNAOS_NOSTORAGE: usb-storage omitted — kernel sees no block device (metal-like no-storage path)");
    } else if hubstorage {
        println!("   UNAOS_HUBSTORAGE: usb-storage attached behind a usb-hub (hub-downstream MSC path)");
        cmd.arg("-drive").arg(format!("if=none,id=stick,format=raw,file={}", stick_image.display()))
           .arg("-device").arg("usb-hub,bus=xhci.0,port=1,id=hub0")
           .arg("-device").arg("usb-storage,bus=xhci.0,port=1.1,drive=stick,bootindex=1");
    } else {
        cmd.arg("-drive").arg(format!("if=none,id=stick,format=raw,file={}", stick_image.display()))
           .arg("-device").arg("usb-storage,bus=xhci.0,drive=stick,bootindex=1");
    }
    cmd.arg("-device").arg("usb-kbd,bus=xhci.0")
       .arg("-device").arg("usb-tablet,bus=xhci.0")
       .arg("-m").arg("1G");

    // U2.5 (UNAOS_USBSERIAL): attach an FTDI FT232 usb-serial device on the xHCI bus. QEMU's
    // `-device usb-serial` emulates an FT232 (VID 0x0403 PID 0x6001, bulk IN 0x81 / OUT 0x02); its
    // chardev is a file at target/ftdi.log, so the kernel's FTDI console driver enumerates it and
    // replays the boot log out bulk-OUT — the metal cable (arriving ~2026-07-08) behaves the same.
    // NOTE: `is_ok()` treats an EMPTY value as SET (the known knob trap) — `UNAOS_USBSERIAL=` is ON.
    if usbserial {
        let ftdi_log = target_dir.join("ftdi.log");
        let _ = std::fs::remove_file(&ftdi_log); // start each run with a fresh capture file
        cmd.arg("-chardev").arg(format!("file,id=ftdi0,path={}", ftdi_log.display()))
           .arg("-device").arg("usb-serial,bus=xhci.0,chardev=ftdi0");
        println!("   U2.5: FTDI usb-serial attached; console capture -> {}", ftdi_log.display());
    }

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
