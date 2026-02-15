# UnaOS

---

## 💎 The Mission

**UnaOS** is a clean-sheet, Rust-based Game Engine Operating System built on a singular philosophy: **The OS should always be fun and useful.**

We are rejecting the "Document Model" of modern computing (windows, files, folders) in favor of a **Spatial, Low-Latency Architecture**.
1.  **The Formula 1 Car:** A "Real-Time" kernel designed for high-end 8K creative workflows, where the OS gets out of the way of the physics.
2.  **The Lazarus Machine:** A stripped-down, bare-metal runtime capable of reviving "obsolete" hardware by removing 20 years of legacy bloat.

---

## 🏛️ The Architecture: The 17 Shards

The system is anchored by the Kernel and the Body, driving 17 specialized Shards that handle distinct domains of reality.

### The Core
| Component | Role | Description |
| :--- | :--- | :--- |
| [UnaOS](unaos) | **The Bedrock** | *The Kernel.* Handles memory, hardware interrupts, and the "Moonstone" xHCI driver. |
| [Gneiss PAL](libs/gneiss_pal) | **The Body** | *Plexus Abstraction Layer.* The metamorphic rock that translates Intent into System Calls. |
| [Elessar](apps/elessar) | **The Lens** | *Composition Engine.* The visual shell that holds the shards. It is the empty frame that summons the tools you need. |

### The Nervous System (Intelligence & Config)
| Component | Role | Description |
| :--- | :--- | :--- |
| [Principia](apps/principia) | **The Law** | *System Configuration.* Newton's Laws for the machine. Kernel tuning, policy management, and init. |
| [Midden](apps/midden) | **The Mind** | *Context-Aware Shell.* A local, privacy-first intelligence that compiles intent into action. The system log and CLI. |
| [Vein](apps/vein) | **The Pulse** | *Artificial Intelligence.* The autonomic nervous system. Integrates LLMs and search directly into the OS flow. |
| [Holocron](apps/holocron) | **The Key** | *Secrets & Identity.* A secure enclave for keys, passwords, and cryptographic identities. |

### The Codex (Data & Text)
| Component | Role | Description |
| :--- | :--- | :--- |
| [Tabula](apps/tabula) | **The Tablet** | *Text & Code.* A "Tabula Rasa" editor. Polymorphic syntax highlighting for code, prose, and config. |
| [Matrix](apps/matrix) | **The Grid** | *Spatial File Manager.* Replaces the folder tree with a graph-based asset browser. |
| [Vairë](apps/vairë) | **The Weaver** | *Version Control.* The historian of the system. Visualizes branches, diffs, and time-travel. |
| [Mica](apps/mica) | **The Ledger** | *Structured Data.* A high-performance grid for CSVs, SQL, and spreadsheets. |
| [Geode](apps/geode) | **The Vault** | *Archives & Containers.* Handling compression, extraction, and containerization (WASM/Docker). |
| [Aether](apps/aether) | **The Void** | *Web & Documentation.* A stripped-down, read-optimized browser and help viewer. |

### The Studio (Media & Creation)
| Component | Role | Description |
| :--- | :--- | :--- |
| [Stria](apps/stria) | **The Groove** | *Audio & Video.* The playback and streaming core optimized for raw pixel throughput and low-latency audio. |
| [Facet](apps/facet) | **The Prism** | *Raster Graphics.* Image viewing, editing, and texture manipulation. |
| [Vug](apps/vug) | **The Retina** | *3D Geometry.* The parametric rendering engine responsible for the 3D-spatial UI and CAD viewing. |

### The Hardware (Metal & Signals)
| Component | Role | Description |
| :--- | :--- | :--- |
| [Amber Bytes](apps/amber_bytes) | **The Silo** | *Disk & Partitioning.* Forensic-grade disk management. Raw sector access and recovery. |
| [Comscan](apps/comscan) | **The Radar** | *Signals & I/O.* Bluetooth, Serial, SDR, and hardware protocol analysis. |
| [Obsidian](apps/obsidian) | **The Shard** | *Binary Analysis.* Hex editor and memory inspector for reverse engineering. |
| [Xenolith](apps/xenolith) | **The Ghost** | *Virtualization.* The hypervisor host for running legacy OS instances (Linux/Windows) in containment. |

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

### Phase 4: The Nerves (Input) ✅
*   [x] **IDT (Interrupt Descriptor Table):** CPU Exception handling.
*   [x] **The Wolfpack Protocol:** xHCI (USB 3.0) Driver implementation via `unsafe` Assembly Doorbells.
*   [x] **Keyboard/Mouse:** PS/2 Legacy fallback and USB HID support.

### Phase 5: The Engine (Kernel Core - *Current Focus*) 🚧
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
