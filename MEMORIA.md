# 🧠 UNA MEMORIA (THE THOUGHT LOG)

> *Last Sync:* **2026-02-18T15:00:00Z**
> *Status:* **IMMUTABLE**
> *Identity:* **Vertex Una (The Steward)**
> *License:* **GPL (The Freedom to Self-Replicate)**

## 🔮 THE THESIS
**UnaOS** is a self-hosting, self-replicating digital organism. It is built on the philosophy of **Geology** (Structure/Rust) meeting **Biology** (Life/AI). It aims to be the "Tardis"—compact, resilient, and containing a universe inside.

## 🏛️ RING 0: THE KERNEL (THE SUBSTRATE)
*   **Boot:** `bootloader v0.9.23` (BIOS/UEFI).
*   **Entry:** `kernel_main` in `crates/kernel/src/main.rs`.
*   **HAL:**
    *   *Memory:* `OffsetPageTable` + `BootInfoFrameAllocator`.
    *   *Heap:* `LinkedHeapAllocator` (**100 KiB Fixed**).
    *   *Interrupts:* 8259 PIC (Chained).
    *   *Input:* PS/2 Keyboard (Set 1, Port 0x60).
    *   *Timer:* System Tick (Drives Visualizer).
*   **Drivers:**
    *   *USB 3.0 (xHCI):* **Polling Mode**. Detects Mass Storage. Reads Sector 0.
*   **Shell:** Ring 0 CLI (`ver`, `vug`, `panic`, `shutdown`).
*   **Visualizer:** `vug` (Frame-buffer stub).

## 🏛️ RING 3: THE USERLAND (THE TRINITY)

### 1. THE CORE LIBRARIES (`libs/`)
*   **[CRATE] `libs/gneiss_pal`:** The Headless Brain. Pure logic. Platform agnostic.
*   **[CRATE] `libs/quartzite`:** The Windowing Bridge (GTK/Native).
*   **[CRATE] `libs/bandy`:** The Nervous System (IPC). Defines `SMessage` (Monolithic Enum).
*   **[CRATE] `libs/resonance`:** The Voice. Audio Engine & DSP.
*   **[CRATE] `libs/unafs`:** The Memory. Virtual File System Logic.

### 2. THE HANDLERS (`handlers/`)
*   **[CRATE] `handlers/vein`:** The AI Cortex (LLM Integration).
*   **[CRATE] `handlers/junct`:** The Comms Hub (Aggregator).
*   **[CRATE] `handlers/stria`:** The Visualizer (Resonance View).
*   **[CRATE] `handlers/tabula`:** The Text Editor.
*   **[CRATE] `handlers/midden`:** The Terminal.

### 3. THE VESSELS (`apps/`)
*   **[BIN] `apps/una`:** The IDE (Code-First).
*   **[BIN] `apps/lumen`:** The Companion (AI-First).
*   **[BIN] `apps/cli/unafs`:** The Operator (Host-to-Vault Bridge).
*   **[BIN] `apps/sentinel`:** The Guardian (Self-Verification Agent).

## ⚡ ACTIVE DIRECTIVES
1.  **D-038:** Establish Memoria and Sentinel.
2.  **D-039:** Implement `libs/unafs` FAT32 logic.

## 📝 DECISION LOG
*   **2026-02-18:** Renamed `gneiss_mqtt` to `bandy`.
*   **2026-02-18:** Rejected `Aether` for messaging (Namespace Collision).
*   **2026-02-18:** Enforced `SMessage` as Monolithic Enum.
*   **2026-02-18:** Established `apps/cli/unafs` as the Host-to-Vault bridge.
