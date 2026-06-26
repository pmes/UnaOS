# kernel

The UnaOS kernel: a bare-metal, `#![no_std]` Rust kernel for x86_64 and aarch64.
It is entered from the [bootloader](../bootloader) with a
[`BootInfo`](../boot-info) and runs everything from interrupt setup through the
drivers and the on-screen console.

## Boot sequence (`src/main.rs`, `kernel_main`)
1. `video::fbcon::init` — framebuffer log sink (mirrors `serial_println!` to the
   screen for serial-less hardware).
2. `init()` — GDT, IDT, local APIC (x86_64) / GIC (aarch64).
3. `arch::memory::init` — heap and page tables.
4. `arch::acpi::init` → `smp::start_aps` → `sched::init` — SMP bring-up and the
   scheduler (x86_64).
5. `arch::pci::init` — enumerate PCI; bring up xHCI and the e1000 NIC.
6. `video::WRITER` / `Screen` — the GUI framebuffer surface.
7. The main loop (on the BSP) services xHCI storage, the network stack
   (`e1000::service_net`), and console input.

## Module map
- `arch/` — per-architecture code: `x86_64/{gdt,idt,interrupts,apic,acpi,smp,sched,percpu,pci,memory}`, `aarch64/`.
- `drivers/` — `xhci/` (USB), `e1000` (NIC), `block` (USB mass storage), `pci`.
- `video/` — `FrameBuffer`, double-buffered `Screen`, `fbcon`.
- `console`, `shell`, `pal`, `vug`, `allocator` — the on-screen console, command
  shell, drawing abstraction, visual demo, and heap allocator.

## Subsystem documentation
- [SMP & scheduler](../../../docs/dev/OS/02_KERNEL_CORE/scheduler.md)
- [Network stack](../../../docs/dev/OS/06_NETWORK_STACK/network_stack.md)
- [USB / xHCI & storage](../../../docs/dev/OS/07_USB_STORAGE/usb_xhci.md)
- [Video / framebuffer](../../../docs/dev/OS/08_VIDEO/framebuffer.md)
- [Boot / HAL](../../../docs/dev/OS/01_BOOT_HAL)

## Build & run
From `unaos/`: `./arroyo check` (type-check both arches), `./arroyo test [secs]`
(headless x86 boot, serial → `target/serial.log`), `./arroyo x86` / `./arroyo arm`
(GUI). `cargo test -p net` runs the network stack's host unit tests.
