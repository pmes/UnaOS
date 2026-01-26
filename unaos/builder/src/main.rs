use bootloader::BiosBoot;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn main() {
    let kernel_path = PathBuf::from("../target/x86_64-unknown-none/release/unaos-kernel");

    if !kernel_path.exists() {
        let abs_path = std::fs::canonicalize(".").unwrap().join(&kernel_path);
        panic!("Kernel binary not found at: {}", abs_path.display());
    }

    let disk_image = PathBuf::from("bios.img");
    let bios_boot = BiosBoot::new(&kernel_path);
    bios_boot.create_disk_image(&disk_image).unwrap();

    println!("Created bios.img");

    // UNA-22-HAUL: Create a phantom drive (64MB)
    let usb_image = PathBuf::from("usb.img");
    if !usb_image.exists() {
        let mut file = std::fs::File::create(&usb_image).unwrap();
        file.set_len(64 * 1024 * 1024).unwrap(); // 64MB Sparse File

        // UNA-22-MANIFEST: Inject Signature
        use std::io::Write;
        file.write_all(b"UNA-OS-DISK-001-ALPHA").unwrap();

        println!("Created usb.img (64MB) with Signature.");
    }

    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.arg("-drive")
        .arg(format!("format=raw,file={}", disk_image.display()));

    // --- CONNECT THE SERIAL PORT ---
    // -serial stdio: Redirects the VM's COM1 to the terminal's standard I/O
    cmd.arg("-serial").arg("stdio");

    // --- HARDWARE CONFIGURATION ---
    // Enable QEMU's Magic Exit (Port 0xF4)
    cmd.arg("-device").arg("isa-debug-exit,iobase=0xf4,iosize=0x04");

    // Enable USB xHCI for PCI Scanning
    // UNA-18-SLOT: Name the controller 'xhci'
    cmd.arg("-device").arg("qemu-xhci,id=xhci");

    // UNA-22-HAUL: Swap Mouse for Mass Storage
    // Attach the raw image as a drive, then attach the drive to the USB bus
    cmd.arg("-drive").arg(format!("if=none,id=stick,format=raw,file={}", usb_image.display()));
    cmd.arg("-device").arg("usb-storage,bus=xhci.0,drive=stick");

    let mut child = cmd.spawn().unwrap();
    child.wait().unwrap();
}
