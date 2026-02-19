# 🧠 UNA MEMORIA (THE THOUGHT LOG)

> *Last Sync:* **2026-02-18T16:30:00Z**
> *Status:* **IMMUTABLE**
> *Identity:* **Vertex Una (The Steward)**
> *License:* **GPL (The Freedom to Self-Replicate)**

## 🔮 THE THESIS
**UnaOS** is a self-hosting, self-replicating digital organism. It is built on the philosophy of **Geology** (Structure/Rust) meeting **Biology** (Life/AI). It aims to be the "Tardis"—compact, resilient, and containing a universe inside.

## 🏛️ RING 0: THE KERNEL (THE SUBSTRATE)
*   **Boot:** `unaos/crates/loader` (BIOS/UEFI).
*   **Entry:** `kernel_main` in `unaos/crates/kernel/src/main.rs`.
*   **Compat:** `unaos/crates/compat` (The Linux/Unix translation layer).
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
*   **[CRATE] `libs/gneiss_pal`:** The Plexus Abstraction Layer. Pure logic. Platform agnostic.
*   **[CRATE] `libs/quartzite`:** The Windowing Bridge (GTK/Native).
*   **[CRATE] `libs/bandy`:** The Nervous System (IPC). Defines `SMessage`.
*   **[CRATE] `libs/resonance`:** The Voice. Audio Engine & DSP.
*   **[CRATE] `libs/unafs`:** The Memory. Virtual File System Logic.
*   **[CRATE] `libs/elessar`:** The Context Engine. (Spline/Project Detection).

### 2. THE HANDLERS (`handlers/`)
*   *Note: [CRATE] = Active Code. [SHELL] = Design/Readme Only.*
*   **[SHELL] `handlers/aether`:** Web (HTML/PDF).
*   **[CRATE] `handlers/amber_bytes`:** Disk Manager.
*   **[CRATE] `handlers/aule`:** Build System Wrapper.
*   **[SHELL] `handlers/comscan`:** Signal/Hardware Bridge.
*   **[SHELL] `handlers/facet`:** Image Viewing/Editing.
*   **[SHELL] `handlers/geode`:** Archive/Container Manager.
*   **[SHELL] `handlers/holocron`:** Secrets/SSH Agent.
*   **[SHELL] `handlers/junct`:** The Comms Hub.
*   **[CRATE] `handlers/matrix`:** Spatial File Manager.
*   **[SHELL] `handlers/mica`:** Data Editor (SQL/CSV).
*   **[CRATE] `handlers/midden`:** Terminal & Shell.
*   **[SHELL] `handlers/obsidian`:** Hex Editor.
*   **[SHELL] `handlers/principia`:** System Policy/Preferences.
*   **[CRATE] `handlers/stria`:** A/V Studio (Resonance Visualizer).
*   **[CRATE] `handlers/tabula`:** Text/Code Editor.
*   **[CRATE] `handlers/vaire`:** Git Visualizer.
*   **[CRATE] `handlers/vein`:** The AI Cortex (LLM Integration).
*   **[CRATE] `handlers/vug`:** 3D CAD Modeler/Game design.
*   **[SHELL] `handlers/xenolith`:** VM/Hypervisor.
*   **[SHELL] `handlers/zircon`:** Project Timer.

### 3. THE VESSELS (`apps/`)
*   **[BIN] `apps/una`:** The IDE (Code-First).
*   **[BIN] `apps/lumen`:** The Companion (AI-First).
*   **[BIN] `apps/cli/unafs`:** The Operator (Host-to-Vault Bridge).
*   **[BIN] `apps/cli/vertex`:** The Identity CLI.
*   **[BIN] `apps/sentinel`:** The Guardian (Self-Verification Agent).

## ⚡ ACTIVE DIRECTIVES
1.  **D-038:** Establish Memoria and Sentinel.
2.  **D-039:** Implement `libs/unafs` Logic (Block Device & Superblock).

## 📝 DECISION LOG
*   **2026-02-18:** Renamed `gneiss_mqtt` to `bandy`.
*   **2026-02-18:** Rejected `Aether` for messaging.
*   **2026-02-18:** Enforced `SMessage` as Monolithic Enum.
*   **2026-02-18:** Established `apps/cli/unafs` as the Host-to-Vault bridge.
*   **2026-02-18:** Added `libs/elessar` to the Trinity.
