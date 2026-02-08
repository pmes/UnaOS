# 🪨 Gneiss PAL: The Bedrock

> *"The Captain gives the order. The Engineer turns the valve. Gneiss connects them."*

**Gneiss** (pronounced "Nice") is the **Plexus Abstraction Layer** for the **UnaOS** ecosystem. It is the metamorphic rock that sits between the raw silicon and the high-level intent of the user.

Its mission is to solve the **"Kirk Paradox"**:
1.  **The Geek (Scotty):** Needs absolute, raw access to the hardware to squeeze out every drop of performance.
2.  **The Captain (Kirk):** Needs a system that adapts instantly to high-level commands without getting bogged down in syntax.

Gneiss provides the **Universal API** that satisfies both.

---

## 🏛 The Philosophy

### 1. Metamorphic Architecture (Write Once, Run Anywhere)
Gneiss defines the "Shape" of the OS capabilities (Files, Network, Graphics) as Rust Traits.
*   **On Linux (Host Mode):** Gneiss maps these traits to standard `libc` and `X11/Wayland` calls.
*   **On UnaOS (Target Mode):** Gneiss maps these traits to raw `unsafe` assembly, `Moonstone` drivers, and direct MMIO.

This allows us to develop the "Captain's Interface" (Midden, Vug, Stria) on a MacBook, and have it run natively on bare metal without changing a single line of logic.

### 2. The "Escape Hatch" (Geek Mode)
Most abstraction layers (Java JVM, .NET) hide the hardware to "protect" the user. Gneiss rejects this.
*   **The Safe Path:** `Gneiss::Display.draw_rect(...)` (Abstract, Safe, Portable).
*   **The Raw Path:** `Gneiss::Display.as_raw_ptr()` (Unsafe, Direct, Dangerous).
We give the Captain the bridge, but we leave the Jefferies Tubes unlocked for the Engineer.

### 3. Intent-Based Computing
Gneiss is not about "Opening Files." It is about "Accessing Assets."
*   **Old Way:** `fopen("/dev/sda1/video.mkv", "rb")`
*   **Gneiss Way:** `Asset::summon("video.mkv").with_priority(Critical)`
Gneiss decides *how* to get it. Maybe it's on disk. Maybe it's in the **Stria Crystal** cache. Maybe it's over the network. The Captain doesn't care; they just want the video.

---

## 🧱 The Shard API

Gneiss exposes the system through **Shards**—autonomous actors that handle specific domains.

```rust
// The Captain's Code (High Level)
// No manual memory management. No specific driver calls.
use gneiss_pal::shards::{Display, Audio};

fn engage_alert() {
    // Gneiss handles the translation to hardware
    Display::set_mood(Color::RedPulse); 
    Audio::announce("Shields Buckling");
}
```

```rust
// The Engineer's Code (Low Level Implementation)
// This lives inside Gneiss, invisible to the Captain.
impl Display for MoonstoneGPU {
    fn set_mood(&self, color: Color) {
        unsafe {
            // Direct MMIO write to the hardware register
            write_volatile(0xB8000 as *mut u32, color.as_u32());
        }
    }
}
```

---

## 🛠 Developer Information

**Gneiss** is the only dependency that `UnaOS`, `Midden`, and `Stria` all share.

**Prerequisites:**
*   Rust `no_std` environment compatibility.
*   `alloc` crate for dynamic memory (Vectors, Box).

**Build Targets:**
```bash
# Build for Host (Linux/macOS - Testing)
cargo build -p gneiss_pal --features "std"

# Build for Target (UnaOS - Metal)
cargo build -p gneiss_pal --target x86_64-unknown-none
```

---

## 🤖 Directives for Agents (The Wolfpack)

**ATTENTION:** If you are an AI agent (J1-J20) contributing to this repository, you must adhere to the following **Gneiss Protocol**:

### 1. The "Abstract First" Rule
Always code against the **Gneiss Traits**, never against the hardware directly (unless you are writing a Driver in `crates/kernel`).
*   ❌ **Bad:** `let file = std::fs::File::open(...)` (Breaks on bare metal).
*   ✅ **Good:** `let asset = Gneiss::Storage::open(...)` (Works everywhere).

### 2. The "Panic" Ban
Gneiss is the bedrock. If Gneiss panics, the ship explodes.
*   **Rule:** Every function must return `Result<T, GneissError>`.
*   **Recovery:** If a driver fails, Gneiss must fallback gracefully (e.g., if GPU fails, fallback to UART console).

---

## 🔮 Roadmap

*   [ ] **Phase 1: The Skeleton** - Define the `Console`, `Alloc`, and `Time` traits.
*   [ ] **Phase 2: The Host** - Implement the `std` backend for Linux development.
*   [ ] **Phase 3: The Metal** - Connect Gneiss to the **Moonstone** Kernel drivers.
*   [ ] **Phase 4: The Network** - Abstract the complexities of TCP/UDP/QUIC into `Gneiss::Comm`.

> *"The rock is solid so the mind can be fluid."*
