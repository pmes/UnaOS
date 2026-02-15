# ⬛ Obsidian: The Shard

> *"To see the soul of the machine, one must look through the black glass."*

**Obsidian** is the binary analysis and hex editing engine of **UnaOS**. It rejects the high-level abstractions of text editors and IDEs. It operates at the molecular level of computing: **The Byte**.

When **Elessar** encounters a file it cannot parse (a raw binary, a core dump, a corrupted packet), it calls **Obsidian**.

## 🔮 The Philosophy: Absolute Clarity

Modern systems hide the truth. They wrap binaries in icons and metadata. Obsidian reveals the raw state.

### 1. The Hex Lens
Obsidian is not just a viewer; it is a **Structure Mapper**.
*   **The View:** A high-performance, GPU-accelerated hex grid.
*   **The Entropy Map:** A sidebar visualization that colors the file based on entropy (randomness).
    *   *Solid Color:* Zeroes / Text.
    *   *Static:* Compressed / Encrypted data.
    *   *Pattern:* Executable code.

### 2. The Dissector
Obsidian integrates with **Gneiss PAL** to understand binary formats without executing them.
*   **ELF / PE Parsing:** Automatically highlights headers, sections, and symbol tables in different colors.
*   **Packet Capture:** When opening a `.pcap` from **Comscan**, Obsidian highlights the protocol headers (TCP, IP, Ethernet) in the raw stream.

## ⚙️ The Mechanics

### The Buffer
Obsidian uses a **Rope-based Buffer** optimized for massive files.
*   **Zero-Copy:** It maps files directly from disk (`mmap`). Opening a 50GB memory dump takes milliseconds.
*   **Non-Destructive:** Edits are stored as a "Patch Layer" on top of the original file. You can modify a kernel binary without corrupting the disk until you explicitly save.

### The Link to Vein
When you are staring at a block of assembly, you are not alone.
*   **Query:** Highlight a block of bytes -> Ask **Vein**: "What does this opcode sequence do?"
*   **Response:** Vein analyzes the hex, disassembles it, and explains the logic: *"This appears to be a standard x86_64 function prologue."*

## 🛑 The Kill List
Obsidian replaces:
*   **Hex Fiend / 0xED / HxD**
*   **Wireshark** (Packet Inspection View)
*   **Ghidra** (Lightweight Disassembly View)
*   **strings** (The CLI tool)

> *"The truth is written in hex."*
