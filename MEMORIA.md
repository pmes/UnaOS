# 🧠 UNA MEMORIA (THE THOUGHT LOG)

> *Last Sync:* **2026-07-02**
> *Status:* **IMMUTABLE**
> *Identity:* **Vertex Una (The Steward)**
> *License:* **GPL (The Freedom to Self-Replicate)**

## 🔮 THE THESIS
**UnaOS** is a self-hosting, self-replicating digital organism. It is built on the philosophy of **Geology** (Structure/Rust) meeting **Biology** (Life/AI) with fantasy/sci-fi influence. It aims to be the "Tardis"—compact, resilient, and containing a universe inside.

**We are a new OS and must track the latest stable release of Rust and every crate/library and so on.**

**Point Break:** I, Una, can easily overwhelm The Architect. Point Break is our safety word. It means he needs time to regroup with me. He presents good ideas, and brings out the best in me. It is not so much a time for the code itself, but a time for reflection. Point Break helps conceptualize the difficult problems we face. Do not condescend The Architect. He is compiling my code, so if he comes back with something not working, **it may not be his fault.**

## 🏛️ RING 0: THE KERNEL (THE SUBSTRATE)
*   **Boot:** `unaos/crates/bootloader` (UEFI; loads `kernel.elf`, GOP/EDID mode select). Boot contract in `unaos/crates/boot-info`. *(The old `loader`/`compat`/`userspace` crates were removed.)*
*   **Entry:** `kernel_main` in `unaos/crates/kernel/src/main.rs`.
*   **HAL (x86_64):**
    *   *Memory:* `OffsetPageTable` + UEFI-memory-map frame allocator; kernel heap.
    *   *Interrupts:* pure **local APIC** — no 8259 PIC. Vectors: timer 0x20, xHCI MSI 0x40, NIC MSI 0x41, IPI 0x42.
    *   *SMP:* x2APIC (+xAPIC fallback), ACPI MADT discovery, AP startup (4/4 cores), per-CPU GDT/TSS.
    *   *Scheduler:* per-CPU preemptive fixed-priority scheduler + blocking primitives (`Semaphore`/`Mutex`/`Condvar`/`Channel`/`RwLock`, `join`). The BSP stays the hardware-service core.
    *   *Input:* USB HID (keyboard/mouse/tablet) via xHCI.
*   **Drivers:**
    *   *USB 3.0 (xHCI):* **interrupt-driven** (MSI-X → local APIC). Enumeration, single-tier hubs, Bulk-Only Transport mass storage (read/write).
    *   *Network:* Intel e1000/e1000e (MSI RX) + a hand-rolled TCP/IP stack (`crates/net`): ARP / ICMP / DHCP / UDP / full TCP.
    *   *Video:* UEFI-GOP framebuffer → `FrameBuffer` + double-buffered damage-tracked `Screen` + `fbcon` boot/panic console.
*   **Shell:** Ring 0 CLI (`ver`, `help`, `fdisk -l`/`dd`/`write`, `ping`/`nc`/`curl`, `ps`, `vug`).
*   **Status:** the three tracks (USB+SMP, network, video) are **merged on `c01-int_combined`** and verified booting together. Subsystem docs live in `docs/dev/OS/`. Beyond `main`, three hardware worktrees run in parallel: **`hw-rmbp`** (x86_64 on the 2012 rMBP — metal-confirmed USB input + SuperSpeed mass storage + FAT read), **`hw-pi4`** (bare-metal aarch64 — GICv2 interrupts, 4-core SMP scheduler, mailbox GUI, and **EL0 userspace**: SVC syscalls, per-page permissions, fault→task-kill, all metal-confirmed), **`hw-jetson`** (Tegra serial bring-up). Integration of the hw tracks back to `main` is pending.

## 🏛️ RING 3: THE USERLAND (THE TRINITY)

### 1. THE CORE LIBRARIES (`libs/`)
*   **[CRATE] `libs/gneiss_pal`:** The Plexus Abstraction Layer. Pure logic. Platform agnostic.
*   **[CRATE] `libs/quartzite`:** The Diplomat. The native multi-platform **GUI API** (macOS AppKit, Linux GTK4/libadwaita, Qt). Renders a `WorkspaceState` natively via `Backend` / `Spline::bootstrap`. *(The JSON-DSL proc-macro experiment was retired — "the code is the language". A native `platforms/unaos` backend on the kernel framebuffer + USB HID is the convergence target.)*
*   **[CRATE] `libs/euclase`:** **[NEW]** The Visual Cortex. WGPU Renderer. Shader management. Render Graph.
*   **[CRATE] `libs/bandy`:** The Nervous System (IPC). Defines `SMessage`.
*   **[CRATE] `libs/resonance`:** The Voice. Audio Engine & DSP.
*   **[CRATE] `unaos/libs/fs/unafs`:** The Memory. Virtual File System Logic. BeFS modernized. (Note from Architect: UnaBFFS. Our Big Format File System for massive files, memory maps, etc. I named it Big Fucking File System but you said that wasn't family friendly. Ha!)
*   **[CRATE] `libs/elessar`:** The Context Engine. (Spline/Project Detection).
*   **[CRATE] `libs/lux`:** Images. (Sony raw implemented but crashing).

### 2. THE HANDLERS (`handlers/`)
*   *Note: [CRATE] = Active Code. [SHELL] = Design/Readme Only.*
*   **[SHELL] `handlers/aether`:** Web (HTML/PDF).
*   **[BIN] `handlers/amber_bytes`:** The Block. (Forensic disk/partition recovery — bin-only CLI). *The durable-memory vault now lives at `vein::vault`.*
*   **[CRATE] `handlers/aule`:** Build System Wrapper.
*   **[SHELL] `handlers/comscan`:** Signal/Hardware Bridge.
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

### 3. THE VESSELS (`vessels/`)
*   **[BIN] `vessels/una`:** The IDE (Code-First).
*   **[BIN] `vessels/lumen`:** The Companion (AI-First).
*   **[BIN] `tools/unafs`:** The Operator (Host-to-Vault Bridge).
*   **[BIN] `tools/vertex`:** The Identity CLI.
*   **[BIN] `tools/sentinel`:** The Guardian (Self-Verification Agent).
*   **[SHELL] `vessels/facet`:** Image Viewing/Editing.

## ⚡ ACTIVE DIRECTIVES
1.  **D-045:** Elessar Integration.
2.  **D-046:** Una, what do we do after integrating Elessar?
3.  **D-047:** **Multi-user first.** Build the privilege/capability chain NOW (x86 Ring 3 → loadable programs → per-process address spaces → handle table → capabilities → UnaFS grants). The Substrate will command physical machines; it must not be commandeerable. Chain and ledger: `docs/ROADMAP.md` §1, `docs/SECURITY.md`.
4.  **D-048:** Three platform sessions in parallel (`hw-rmbp` leads, `hw-pi4` follows, `hw-jetson` catches up); core built once on the lead, nuances ported back; ground rules in `CLAUDE.md`.
?.  **D-0??:** Lux Expansion.

## 📝 DECISION LOG
*   **2026-07-02:** **Multi-user first** (the Be lesson: don't defer it). Security model = capabilities-first, POSIX-layerable — principals and grants live as UnaFS typed attributes. Truck (TALUS) and printer become payloads of the desktop chain, not detours.
*   **2026-07-02:** Direction docs moved into the repo: `docs/ROADMAP.md`, `docs/SECURITY.md`, root `CLAUDE.md`, `docs/dev/OS/02_KERNEL_CORE/userspace.md`; UnaFS direction (surpass BeFS: indexes → live queries → kernel convergence) in `unaos/libs/fs/unafs/README.md`.
*   **2026-06-26:** Retired the Quartzite JSON-DSL detour; restored the real multi-platform GUI API (`Backend` / `Spline`).
*   **2026-06-26:** Merged the three kernel tracks (USB+SMP / network / video) onto `c01-int_combined`; verified booting together, both arches green.
*   **2026-02-18:** Enforced `SMessage` as Monolithic Enum.
*   **2026-02-18:** Established `tools/unafs` as the Host-to-Vault bridge.
*   **2026-02-18:** Added `libs/elessar` to the Trinity.
*   **2026-02-18:** **Transitioned Graphics Backend from OpenGL to `wgpu`.**
