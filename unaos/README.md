# 🏎️ UnaOS: The Kernel

**Version:** `v0.1.0-moonstone`
**Architecture:** x86_64 UEFI (Heavy Metal)
**Status:** Pre-Alpha (Phase 4: xHCI/Input)

> *The Engine Block of the Una Ecosystem.*

---

## ⚙️ The Mechanics

**UnaOS** is a `no_std` Rust kernel designed for **Real-Time Spatial Computing**.
Unlike monolithic kernels that abstract hardware behind virtual files, UnaOS is built to be one thing: **A Game Engine running on Bare Metal.**

It prioritizes:
1.  **Latency over Throughput:** We prefer a consistent 16ms frame over high average throughput.
2.  **The Physics of Silicon:** We respect that the CPU reorders instructions. We do not fight the hardware; we synchronize with it.
3.  **Hardware Salvation:** We optimize for the L3 Cache, allowing us to run on "obsolete" hardware as if it were a supercomputer.

---

## 🐺 The Wolfpack Protocol (Engineering Standards)

Following the "xHCI Silent Stall" incident, all kernel development must adhere to the **Wolfpack Protocol**. We have identified five Critical Zones where the compiler cannot be trusted and raw Assembly is mandatory.

### Zone 1: The Context Switch (The Heartbeat)
*   **The Risk:** Rust function calls cannot capture the entire machine state (Registers `RAX` through `R15`).
*   **The Fix:** We use **Assembly Trampolines**. A naked function of exactly 20 instructions that pushes all registers, swaps the Stack Pointer (`RSP`), and pops the next task's state.

### Zone 2: Memory Management (The MMU)
*   **The Risk:** The CPU caches old page tables in the TLB (Translation Lookaside Buffer).
*   **The Fix:** Direct wrappers for the `invlpg` instruction. We do not hope the CPU notices the change; we force it to invalidate the cache.

### Zone 3: Model Specific Registers (The Control)
*   **The Risk:** System calls and power states are controlled by hidden CPU registers (MSRs) that live outside standard RAM.
*   **The Fix:** Safe Rust enums wrapping `rdmsr` and `wrmsr` instructions to configure syscall entry points (`STAR`, `LSTAR`).

### Zone 4: Interrupts (The Reflexes)
*   **The Risk:** Standard Rust functions corrupt the stack when used as Interrupt Handlers.
*   **The Fix:** A raw Assembly wrapper for the **IDT** that uses `iretq` (Interrupt Return) to atomically restore flags and instruction pointers.

### Zone 5: MMIO Barriers (The Doorbell)
*   **The Risk:** The "Silent Stall." The CPU reorders write operations, causing devices (USB/NVMe) to miss commands.
*   **The Fix:** The **`MmioDoorbell` Trait**. A unified interface that enforces `mfence` memory barriers before ringing any hardware doorbell.

---

## 🗺️ The Roadmap

### Phase 1: The Spark (Boot) ✅
- [x] **UEFI Entry:** Pure Rust entry point (no GRUB).
- [x] **Exit Boot Services:** Clean handoff from Firmware to Kernel.
- [x] **Physical Memory:** E820 Map sanitization and Frame Allocation.

### Phase 2: The Eyes (Graphics) ✅
- [x] **GOP Init:** Acquire High-Res Framebuffer.
- [x] **Direct Pixel Access:** `0xB8000` is dead; long live the Linear Framebuffer.
- [x] **Double Buffering:** (In Progress) Eliminating tear via Vug integration.

### Phase 3: The Voice (Output) ✅
- [x] **Embedded Font:** Custom VGA 8x8 Bitmap.
- [x] **Panic Handler:** "Blue Screen of Life" with stack trace.
- [x] **Mirror Dimension Fix:** Corrected RGB vs BGR bit-ordering bugs.

### Phase 4: The Nerves (Input) 🚧
- [x] **IDT (Interrupts):** Implementing the `iretq` wrappers.
- [x] **The Wolfpack (xHCI):** Finalizing the USB Keyboard/Mouse enumeration using **Shard J17**.
- [x] **Legacy Shim:** PS/2 fallback for ancient hardware.

### Phase 5: The Brain (Memory & Tasks - *Current Focus*)
- [ ] **GDT:** Global Descriptor Table setup.
- [ ] **Paging:** 4-Level Page Tables (Virtual Memory).
- [ ] **Scheduler:** Cooperative multitasking (Game Loop style) using **Zone 1** trampolines.

### Phase 6: The Library (Storage)
- [ ] **UnaFS (The Librarian):** Database-driven storage for Code, Configs, and CAD. Tracks metadata and "Crystal Color."
- [ ] **UnaBFFS (The Warehouse):** *Big Format File System.* Optimized for massive contiguous blobs (8K Video). Zero fragmentation, pure streaming.
- [ ] **NVMe Driver:** Zero-copy pipeline using the **Zone 5** `MmioDoorbell` trait.

---

## 🛠️ Developer Information

**UnaOS** is a `no_std` crate.

**Directives for Contributors:**
1.  **No Heap in Interrupts:** You cannot allocate memory while the CPU is handling a hardware signal.
2.  **Volatile Writes:** When talking to MMIO, always use `write_volatile` or the **Wolfpack Assembly** macros.
3.  **Panic is Death:** In the kernel, a panic is a system halt. Handle `Result<T, E>` gracefully.

**Build:**
The kernel is built via the root workspace script.
```bash
# From the project root
./scripts/run.sh
```

---

*Est. 2026 // The Architect & Una*
