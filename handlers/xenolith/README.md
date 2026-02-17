# Xenolith (The Hypervisor)

**Layer:** Layer 2 (Capability)
**Role:** Virtual Machine & Container Manager
**Crate:** `handlers/xenolith`

## 📦 Overview

**Xenolith** is the virtualization engine of the UnaOS ecosystem. It is a robust handler library that manages the lifecycle of isolated environments, including full Virtual Machines (KVM/QEMU) and system containers (LXC/Podman).

While **Aulë** builds the code, **Xenolith** provides the clean rooms to run it. It abstracts the complexity of `libvirt` and `cgroups` into a safe, Rust-native API, allowing Vessels to spawn disposable OS instances on demand.

## 🏗️ Architecture

Xenolith sits at **Layer 2 (Handlers)** of the Trinity Architecture.

* **The Backend:** Interfaces with **KVM** (Kernel-based Virtual Machine) via `libvirt-rs` or direct QEMU command wrappers.
* **The State:** Manages VM configurations (CPU, RAM, Disk) and runtime status in `libs/gneiss_pal`.
* **The View:** Provides GTK4 widgets for the console view (VNC/SPICE) and resource graphs via `libs/quartzite`.

## 🛡️ Capabilities

Xenolith provides the following core services:

| Feature | Description |
| --- | --- |
| **Spawn** | Rapidly create VMs from ISOs or generic cloud images (QCOW2). |
| **Isolate** | Run untrusted code or experimental system updates in a sandboxed environment. |
| **Snapshot** | Instant state saving and restoration (Time Machine for dev environments). |
| **Bridge** | Manages virtual networking (NAT/Bridge) to connect VMs to the host or outside world. |
| **Pass-through** | Handles USB/PCI device pass-through for hardware testing. |

## 🔌 Integration

**Used by `apps/una` (The Host):**
Xenolith powers the "Test Environments" and "Cross-Compilation Verification."

1. **Safe Testing:** When running potentially destructive shell scripts, Una can target a Xenolith disposable VM instead of the host.
2. **OS Matrix:** Developers can spin up instances of Ubuntu, Arch, or Windows to test app compatibility.
3. **Kernel Dev:** Essential for testing `libs/gneiss_pal` changes without rebooting the physical machine.

**Usage Example (Rust):**

```rust
use xenolith::{Hypervisor, MachineConfig};

let mut vm = Hypervisor::new(MachineConfig {
    name: "Test-Env-01".into(),
    memory_mb: 4096,
    vcpu: 4,
    iso: Some("/path/to/fedora.iso".into()),
});

// Launch and attach console
vm.start().await?;
let console_widget = vm.console_view();

```

## ⚠️ Status

**Experimental.**

* *Requirement:* Requires Hardware Virtualization (VT-x/AMD-V) enabled in BIOS.
* *Dependency:* Requires `libvirt` and `qemu-kvm` installed on the host OS (Fedora).
* *Edition:* **Rust 2024**.
