# 📜 The Codex: The UnaOS System Canon

> *"We do not guess. We check the Codex."*
> *Last Updated: Platinum Era (S35)*

## 1. The Core Philosophy
*   **The Mission:** A clean-sheet, Rust-based **Game Engine OS**. We prioritize latency, spatialization, and joy over legacy compatibility.
*   **The Architecture (The Two Kernels):**
    *   **Ring 0 (Infrastructure):** `unaos`. The Power Grid. Minimalist hardware abstraction. It ensures electrons flow and memory is safe.
    *   **Ring 3 (The Library):** `gneiss_pal`. The Institution. The User Space Kernel. It holds the universal logic for physics, networking, and rendering.
*   **The Interface:** **Polymorphism**.
    *   **Project Mode:** **Elessar** binds multiple Handlers (Code + Terminal + Git) into a workspace.
    *   **Solo Mode:** Opening a single file creates a "Micro-Project." A video file opens **Stria**, but with full metadata capabilities (A-B loops, Bookmarks) because even consumption is an active process.

## 2. The Handler Manifest (The 20)

**Status:** **LOCKED**.

| Handler | Domain | The "App" It Replaces | Technical Role |
| :--- | :--- | :--- | :--- |
| **Aether** | **Web** | Chrome, Acrobat | **The Reader.** Read-only renderer for HTML, Markdown, PDF. No JIT JS. |
| **Amber Bytes** | **Disks** | Disk Utility | **The Block.** Partitioning (GPT), Formatting, Block-level recovery. |
| **Aulë** | **Forge** | Cargo, Make | **The Builder.** Manages compilation, assets, and packaging. |
| **Comscan** | **Signals** | Terminal, Pronterface | **IO Bridge.** Serial, GPIO, Bluetooth, SDR. Controls hardware. |
| **Facet** | **Images** | Photoshop, Preview | **The Canvas.** Raster/Vector engine. GPU-accelerated texture editing. |
| **Geode** | **Archives** | Docker, WinZip | **The Vault.** Container engine. Manages immutable snapshots (`.geode`). |
| **Holocron** | **Secrets** | 1Password, GPG | **The Key.** Keyring, SSH Agent, Wallet, Biometric Auth. |
| **Matrix** | **Files** | Finder, Trello | **Spatial Asset Manager.** Visualizes Files and Tasks in a 3D/2D grid. |
| **Mica** | **Data** | Excel, SQL | **The Ledger.** Structured Data Engine. SQL, CSV, Parquet editor. |
| **Midden** | **Shell** | Bash, PowerShell | **The CLI.** Manages `stdin`/`stdout` and the Command History Graph. |
| **Obsidian** | **Binary** | Hex Fiend | **The Scope.** Hex Editor, Disassembler, Binary Analysis. |
| **Principia** | **System** | Settings, Config | **The Architect.** Policy Engine. Manages `init`, Boot Args, and Hardware Profiles. |
| **Junct** | **Colab** | Discord, Slack, ThunderBird, Zoom | **The Receiver.** Aggregating Matrix, Email, IRC, and RSS into a single "Stream" rather than fragmented apps. |
| **Stria** | **A/V** | Premiere, VLC | **The Studio.** DSP Graph. Playback with non-destructive A-B looping & bookmarking. |
| **Tabula** | **Text** | VS Code, Word | **The Quill.** `tree-sitter` powered text and code manipulation. |
| **Vairë** | **Repos** | GitKraken | **The Loom.** Visualizes the Git DAG (Directed Acyclic Graph). |
| **Vein** | **AI** | ChatGPT, Copilot | **The Mind.** Context-aware intelligence and UI orchestration & Provider Abstraction (Local/Cloud). |
| **Vug** | **3D** | Fusion 360, Cura | **The Sculptor.** CAD viewing, editing, and **CAM/Slicing**. |
| **Xenolith** | **VMs** | VirtualBox | **The Bridge.** Hypervisor Frontend for running Guest OSs. |
| **Zircon** | **Time** | Calendars, Scheduling, Gantt Charts. | **The Chronometer.** Integrates with Matrix to show project milestones and deadlines in a timeline view. |

## 3. Gneiss PAL: The Great Library

**Gneiss** (`libs/gneiss_pal`) is the User Space Kernel. It prevents "App Silos."
When **Vug** needs to slice a 3D model, it uses the geometry engine in Gneiss. When **Comscan** needs to send G-Code, it uses the serial stack in Gneiss.

*   **`src/fs`**: The **UnaFS** client. Indexing and Metadata.
*   **`src/net`**: The Network/Signal Stack. TCP/IP and Serial/Bluetooth.
*   **`src/geo`**: The Geometry Kernel. B-Rep and Mesh math.
*   **`src/dsp`**: The Signal Processing Graph. Audio and Video codecs.
*   **`src/flux`**: The Windowing Logic. Calculates layout and occlusion.

## 4. The Elessar Protocol (The Binder)

**Elessar** manages the **Context**. It creates a "binder" around your project.

### The Dynamic Layout
*   **The Project:** You open a folder. Elessar inspects the contents.
*   **The Morph:**
    *   **Code Project:** Binds **Tabula** (Center) + **Midden** (Bottom) + **Vairë** (Sidebar).
    *   **CAD Project:** Binds **Vug** (Center) + **Comscan** (Sidebar) + **Aulë** (Background).
*   **The Interaction:**
    *   **Drag & Drop Intent:** Dragging a window to the edge triggers a "Sidebar" morph.
    *   **Voice/Text Control:** "Vein, move the terminal to the right."

## 5. The Hardware Protocol (CNC/3D)
We kill the middleman.
1.  **Design:** **Vug** renders the model and calculates the toolpath (Slicing/CAM) via Gneiss.
2.  **Control:** **Comscan** takes the stream and pumps it directly to the hardware via USB/Serial.
3.  **Result:** No file exporting. No "Slicer App." Just **Design -> Make**.

## 6. The Wolfpack Protocol (Kernel Zones)

The Kernel (`unaos`) is divided into strict safety zones.

*   **Zone 1: The Gateway.** Assembly Trampolines. The only way in or out of Ring 0.
*   **Zone 2: The Map.** Memory Management. `invlpg` wrappers and Page Table manipulation.
*   **Zone 3: The Scream.** Interrupt Descriptor Table (IDT). Handling hardware exceptions and panics.
*   **Zone 4: The Pulse.** The Scheduler. Context switching logic.
*   **Zone 5: The Fence.** **`MmioDoorbell` Trait**. All hardware IO must be fenced (`mfence`) to prevent race conditions.
