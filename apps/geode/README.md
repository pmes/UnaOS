# 🥚 Geode: The Vault

> *"Pressure makes diamonds. Compression makes the universe fit in your pocket."*

**Geode** is the archival and containerization engine of **UnaOS**. It rejects the clunky, ad-ware ridden "Zip Utilities" of the past. It is not just a compressor; it is the fundamental way we package and distribute software.

**Geode** is the physics of efficient storage.

## 📦 The Philosophy: Atomic Units

Modern software is bloated. **Geode** is designed to strip away the emptiness and encapsulate the value.

### 1. The Archive (Compression)
Geode handles the standard formats, but prefers the modern, high-speed algorithms of the future.
*   **Zstd / LZ4:** Prioritizes decompression speed so archives open instantly, like folders.
*   **Transparent Access:** Browse inside archives without extracting them. Stream a video from a `.zip` without waiting.
*   **Deduplication:** When creating backups, Geode identifies identical blocks across files to save massive amounts of space.

### 2. The Container (Isolation)
Geode is the runtime for packaged applications. It replaces the need for heavy virtualization (Docker) for most tasks.
*   **WASM Capsules:** Execute WebAssembly binaries in a secure, sandboxed environment with near-native speed.
*   **Dependency Locking:** Freeze an application's libraries into a single, immutable Geode file. It runs the same today as it will in ten years.

## ⚙️ The Mechanics

### The Crystal Structure
Geode archives are structured like databases, not just streams of bytes. This allows for random access and metadata queries without reading the whole file.

### The Seal
Every Geode archive is cryptographically signed by **Holocron** upon creation.
*   **Tamper-Proof:** You know exactly who created the package and if it has been altered.
*   **The Manifest:** A clear, human-readable list of contents and permissions required to run the contained software.

## 🛑 The Kill List
Geode replaces:
*   **WinZip / 7-Zip / The Unarchiver**
*   **tar / gzip** (The command line tools are wrapped by Geode)
*   **Docker Desktop** (For lightweight containerization)
*   **AppImage / Flatpak**
