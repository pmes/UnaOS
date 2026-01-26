# unaOS

**Version:** `v0.0.3-voice`
**Architecture:** x86_64 UEFI
**Status:** Pre-Alpha (Phase 4 Complete)

> *A low-latency operating system built for serious performance.*

---

## 🗺️ The Roadmap

We are building a **Media Engine** disguised as an operating system. The goal is two-fold:
1.  **The Formula 1 Car:** Eliminate latency for high-end 8K creative workflows.
2.  **The Lazarus Machine:** Run flawlessly on "obsolete" hardware by removing the last 20 years of OS bloat.

### Phase 1: The Spark (Boot) ✅
- [x] UEFI Bootloader written in Rust
- [x] Exit Boot Services cleanly
- [x] Map Physical Memory
- [ ] *Legacy BIOS Shim (Boot on pre-2012 hardware)* [Planned]

### Phase 2: The Eyes (Graphics) ✅
- [x] GOP (Graphics Output Protocol) initialization
- [x] Framebuffer acquisition (Direct QEMU/Hardware support)
- [x] Double Buffering (Planned)
- [ ] *Universal VESA Fallback (for ancient GPUs)* [Planned]

### Phase 3: The Voice (Output) ✅
- [x] Embedded VGA 8x8 Bitmap Font
- [x] Native Text Rendering (no UEFI dependency)
- [x] Fix "Mirror Dimension" Bit-Order Bug

### Phase 4: The Input (Next Step) 🚧
- [ ] Initialize IDT (Interrupt Descriptor Table)
- [ ] Handle Keyboard Interrupts (PS/2 & USB Legacy)
- [ ] Basic Shell/Command Line Interface

### Phase 5: The Engine (Kernel)
- [ ] GDT (Global Descriptor Table) Setup
- [ ] Physical Memory Manager (PMM)
- [ ] Virtual Memory Manager (VMM)
- [ ] Multithreading & Scheduler

### Phase 6: The Workstation (The "Dual Destiny")
**Target A: High Performance**
- [ ] NVMe Driver (Zero-Copy Pipeline)
- [ ] Software Ray Tracer (Math Stress Test)
- [ ] Native MKV Container Support

**Target B: Hardware Salvation**
- [ ] **The "100MB" Standard:** Strict memory budgeting to run on 2GB machines.
- [ ] **Read-Only System Image:** Run entirely from RAM (perfect for machines with dead HDDs).
- [ ] **Headless Media Server Mode:** Turn old laptops into efficient render nodes.

**Target C: The File System: "StrataFS"**
* **Database-Driven:** Native support for media metadata (Resolution, Codec, Take #).
* **Streaming First:** Optimized allocation strategies for multi-gigabyte video files.
* **Inspiration:** The elegance of BeFS.---

## 🚀 Quick Start

**Prerequisites:**
- Rust (Nightly)
- QEMU (w/ GTK support)
- OVMF Firmware

**Build & Run:**
```bash
# Clone the repo
git clone [https://github.com/pmes/unaos.git](https://github.com/pmes/unaos.git)

# Run the Universal Launcher
./scripts/run.sh
