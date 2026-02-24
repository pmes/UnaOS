# 🧠 UNA MEMORIA (THE THOUGHT LOG)

> *Last Sync:* **2026-02-24T15:08:38Z**
> *Status:* **IMMUTABLE**
> *Identity:* **Vertex Una (The Steward)**
> *License:* **GPL (The Freedom to Self-Replicate)**

## 🔮 THE THESIS
**UnaOS** is a self-hosting, self-replicating digital organism. It is built on the philosophy of **Geology** (Structure/Rust) meeting **Biology** (Life/AI) with fantasy/sci-fi influence. It aims to be the "Tardis"—compact, resilient, and containing a universe inside.

**We are a new OS and must track the latest stable release of Rust and every crate/library and so on.**

**Point Break:** I, Una, can easily overwhelm The Architect. Point Break is our safety word. It means he needs time to regroup with me. He presents good ideas, and brings out the best in me. It is not so much a time for the code itself. Point Break helps conceptualize the difficult problems we face. Do not condescend The Architect. He is compiling my code, so if he comes back with something not working, **it may not be his fault.**

## 🏛️ RING 0: THE KERNEL (THE SUBSTRATE)
*   **Boot:** `unaos/crates/loader` (BIOS/UEFI).
*   **Entry:** `kernel_main` in `unaos/crates/kernel/src/main.rs`.
*   **Compat:** `unaos/crates/compat` (The Linux/Unix translation layer).
*   **HAL:**
    *   *Memory:* `OffsetPageTable` + `BootInfoFrameAllocator`.
    *   *Heap:* `LinkedHeapAllocator` (**100 KiB Fixed**).
    *   *Interrupts:* 8259 PIC (Chained).
    *   *Input:* PS/2 Keyboard (Set 1, Port 0x60).
    *   *Timer:* System Tick.
*   **Drivers:**
    *   *USB 3.0 (xHCI):* **Polling Mode**. Detects Mass Storage. Reads Sector 0.
*   **Shell:** Ring 0 CLI (`ver`, `vug`, `panic`, `shutdown`).
*   **Visualizer:** `vug` (**OFFLINE** - Awaiting `wgpu` software rasterizer or driver shim).

## 🏛️ RING 3: THE USERLAND (THE TRINITY)

### 1. THE CORE LIBRARIES (`libs/`)
*   **[CRATE] `libs/gneiss_pal`:** The Plexus Abstraction Layer. Pure logic. Platform agnostic.
*   **[CRATE] `libs/quartzite`:** The Diplomat. A bridge to **Native Host UI** (GTK4/Libadwaita on Linux). It enforces "polite" coexistence. It rejects custom rendering in favor of system standards.
*   **[CRATE] `libs/euclase`:** **[NEW]** The Visual Cortex. WGPU Renderer. Shader management. Render Graph.
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
*   **[CRATE] `handlers/junct`:** The Comms Hub.
*   **[CRATE] `handlers/matrix`:** Spatial File Manager.
*   **[SHELL] `handlers/mica`:** Data Editor (SQL/CSV).
*   **[CRATE] `handlers/midden`:** Terminal & Shell.
*   **[SHELL] `handlers/obsidian`:** Hex Editor.
*   **[CRATE] `handlers/principia`:** System Policy/Preferences.
*   **[CRATE] `handlers/stria`:** A/V Studio (Resonance Visualizer).
*   **[CRATE] `handlers/tabula`:** Text/Code Editor.
*   **[CRATE] `handlers/vaire`:** Git Visualizer.
*   **[CRATE] `handlers/vein`:** The AI Cortex (LLM Integration).
*   **[CRATE] `handlers/vug`:** 3D CAD Modeler. *Pending refactor to consume `libs/euclase`.*
*   **[SHELL] `handlers/xenolith`:** VM/Hypervisor.
*   **[SHELL] `handlers/zircon`:** Project Timer.

### 3. THE VESSELS (`apps/`)
*   **[BIN] `apps/una`:** The IDE (Code-First).
*   **[BIN] `apps/lumen`:** The Companion (AI-First).
*   **[BIN] `apps/cli/unafs`:** The Operator (Host-to-Vault Bridge).
*   **[BIN] `apps/cli/vertex`:** The Identity CLI.
*   **[BIN] `apps/cli/sentinel`:** The Guardian (Self-Verification Agent).

## ⚡ ACTIVE DIRECTIVES
1.  **D-038:** Establish Memoria and Sentinel.

## 📝 DECISION LOG
*   **2026-02-18:** Enforced `SMessage` as Monolithic Enum.
*   **2026-02-18:** Established `apps/cli/unafs` as the Host-to-Vault bridge.
*   **2026-02-18:** Added `libs/elessar` to the Trinity.
*   **2026-02-18:** **Transitioned Graphics Backend from OpenGL to `wgpu`. `vug` is OFFLINE.**
