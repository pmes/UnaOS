# UnaOS

**Version:** `v0.1.0-moonstone`
**Architecture:** x86_64 UEFI (Heavy Metal)
**Status:** Pre-Alpha (Phase 4: Input/xHCI)

> *The Game Engine Operating System.*

---

## 💎 The Mission

**UnaOS** is a clean-sheet, Rust-based operating system built on a singular philosophy: **The OS should always be useful.**

We are rejecting the "Document Model" of modern computing (windows, files, folders) in favor of a **Spatial, Low-Latency Architecture**.
1.  **The Formula 1 Car:** A "Real-Time" kernel designed for high-end 8K creative workflows, where the OS gets out of the way of the physics.
2.  **The Lazarus Machine:** A stripped-down, bare-metal runtime capable of reviving "obsolete" hardware by removing 20 years of legacy bloat.

---

## 🏛️ The Architecture: The Una Ecosystem

The system is composed of specialized shards, each with a distinct role:

| Component | Role | Description |
| :--- | :--- | :--- |
| **UnaOS** | **The Bedrock** | The Kernel. Handles memory, hardware interrupts, and the "Moonstone" xHCI driver. |
| **Gneiss PAL** | **The Body** | *Plexus Abstraction Layer.* The metamorphic rock that translates Intent into System Calls. |
| **Midden** | **The Mind** | *Context-Aware Shell.* A local, privacy-first intelligence that compiles intent into action, manages history and predicts commands. |
| **Elessar** | **The Lens** | *Polymorphic Editor.* A "Vision Stone" that transmutes its interface (Code, CAD, NLE) based on the asset type. |
<<<<<<< HEAD
| **Aulë** | **The Smith** | *Repository Forge.* The workspace manager that treats the Monorepo as a single, divine structure. |
| **Stria** | **The Groove** | *Media Engine.* The playback and streaming core optimized for raw pixel throughput. |
| **Vug** | **The Retina** | *Geometry Kernel.* The parametric rendering engine responsible for the 3D-spatial UI. |
| **Vein** | **The Pulse** | *Autonomic Nervous System.* The Cloud Infrastructure that builds and verifies the system while we sleep. |
| **Aulë** | **The Hand** | *Tooling.* The Vala of your Forge, archivist of your creations, or you can stick with git. |
=======
| **Vairë** | **The Weaver** | *Repository Historian.* Every change captured--the archivist of your creations, or you can stick with git. |
| **Stria** | **The Groove** | *Media Engine.* The playback and streaming core optimized for raw pixel throughput. |
| **Vug** | **The Retina** | *Geometry Kernel.* The parametric rendering engine responsible for the 3D-spatial UI. |
| **Vein** | **The Pulse** | *Autonomic Nervous System.* The Cloud Infrastructure that builds and verifies the system while we sleep. |
>>>>>>> origin/j8-vein-s33-gneiss-plexus-9431329697366953615
| **Amber Bytes**| **The Silo** | *Forensic Recovery.* The low-level disk manager responsible for raw sector access and disaster recovery. |

---

## 🗺️ The Roadmap

### Phase 1: The Spark (Boot) ✅
*   [x] **UEFI Bootloader:** Pure Rust implementation (no GRUB).
*   [x] **Exit Boot Services:** Clean handoff from Firmware to Kernel.
*   [x] **Physical Memory Map:** E820 traversal and frame allocation.

### Phase 2: The Retina (Graphics) ✅
*   [x] **GOP Initialization:** High-resolution framebuffer acquisition.
*   [x] **Direct Pixel Access:** Writing raw color bytes to video memory.
*   [x] **Vug Protocol:** Basic geometric primitive rendering.

### Phase 3: The Voice (Output) ✅
*   [x] **Embedded Typography:** Custom VGA 8x8 Bitmap Font.
*   [x] **Native Text Rendering:** No dependency on UEFI `ConOut`.
*   [x] **Panic Handler:** "Blue Screen of Life" visual debugging.

### Phase 4: The Nerves (Input) 🚧
*   [x] **IDT (Interrupt Descriptor Table):** CPU Exception handling.
*   [x] **The Wolfpack Protocol:** xHCI (USB 3.0) Driver implementation via `unsafe` Assembly Doorbells.
*   [x] **Keyboard/Mouse:** PS/2 Legacy fallback and USB HID support.

### Phase 5: The Engine (Kernel Core - *Current Focus*)
*   [ ] **GDT & TSS:** Stack switching and privilege levels (Ring 0 vs Ring 3).
*   [ ] **Memory Manager:** Paging, Virtual Memory, and Heap Allocation.
*   [ ] **Multithreading:** Cooperative multitasking scheduler.

### Phase 6: The Workstation (Target Goals)
*   **UnaFS (The Librarian):** A database-driven file system for small, high-value assets (Code, Configs, CAD). It tracks metadata, version history, and "Crystal Color" status.
*   **UnaBFFS (The Warehouse):** *Big Format File System.* Optimized for massive contiguous blobs (8K Video, Disk Images). No fragmentation, just raw streaming speed.
*   **Elessar Context:** The editor detects the file type and the "Crystal Status." If you edit a tracked system file, Midden alerts you (Amber/Red) before you break the build.
*   **Headless Mode:** Turn any x86_64 laptop into a dedicated **Stria** render node.

---

*Est. 2026 // The Architect & Una*
