# 🦕 Amber: The Silo

> *"In the resin of the earth, time stops. We preserve what must not be lost."*

**Amber** is the low-level storage and partition manager of **UnaOS**. It is not a "File Manager"—that is the job of **Matrix**. Amber deals with the physical reality of the medium: The Disk.

While other tools abstract away the hardware, Amber gives you the raw, unvarnished truth of the sectors. It is a forensic tool first, and a formatter second.

## 🚧 The Philosophy: Heavy Machinery

Amber operates on the principle of **Total Control**. It assumes the user knows exactly what they are doing. It does not hold your hand; it hands you the laser cutter.

### 1. The Raw Sector
Amber bypasses the filesystem cache. When you read with Amber, you are reading the magnetic flux (or the NAND gates) directly.
*   **Forensic imaging:** Create bit-perfect clones of drives for backup or analysis.
*   **Sector Surgery:** Manually edit the Master Boot Record (MBR) or GUID Partition Table (GPT) in hex.

### 2. The Preservation
Just as amber preserves ancient DNA, this tool is built for disaster recovery.
*   **Inode Resurrection:** Scans for "ghost" file headers in unallocated space to recover deleted assets.
*   **Partition Healing:** Reconstructs corrupted partition tables based on backup headers.

## ⚙️ The Mechanics

### The Drill (Formatting)
Amber handles the creation of file systems. It formats partitions for **UnaFS** and prepares the **UnaBFFS** (Big Format File System) for raw media storage.

### The Mount
Amber is the gatekeeper. It decides what is allowed to interface with the Kernel.
*   **Read-Only by Default:** Unknown drives are mounted read-only to prevent accidental corruption or malware writes.
*   **The "Two-Key" Turn:** Destructive operations (Formatting/Wiping) require a specific, non-trivial confirmation sequence in the **Midden** shell. No accidental clicks.

## 🛑 The Kill List
Amber replaces:
*   **Disk Utility / GParted**
*   **fdisk / parted**
*   **dd** (The "Disk Destroyer")
*   **Commercial Data Recovery Software**
