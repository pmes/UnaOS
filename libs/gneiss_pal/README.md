# 🗿 Gneiss Plexus Abstraction Layer: The Great Library

> *"The Infrastructure of Society allows for Libraries."*

**Gneiss** (pronounced "Nice") is the User Space Kernel of the Una Operating System. It is the **Plexus Abstraction Layer** that sits between the raw silicon and the high-level intent of the user.

In a functioning society, the government (The Kernel) provides the roads and power grid. But you do not go to the government to learn, to create, or to collaborate. You go to **The Library**.

In traditional operating systems, every application is an island. They each bring their own physics engine, their own windowing toolkit, and their own way of talking to the hardware. This is inefficient. It is lonely.

**Gneiss** changes this. It is the centralized Institution where our "Handlers" (Applications) go to get their work done.

---

## 🏛 The Philosophy: The Kirk Paradox

Gneiss is designed to solve the fundamental tension in OS design:

1.  **The Engineer (Scotty):** Needs absolute, raw access to the hardware to squeeze out every drop of performance.
2.  **The Captain (Kirk):** Needs a system that adapts instantly to high-level commands without getting bogged down in syntax.

Gneiss provides the **Universal API** that satisfies both. We give the Captain the bridge, but we leave the Jefferies Tubes unlocked for the Engineer.

---

## 📚 The Handlers: A New Paradigm

Because Gneiss handles the heavy lifting, our applications are not bloated silos. They are lightweight, specialized tools that empower your creative flow.

### **Vug** (The Sculptor)
Vug is not just a 3D viewer; it is the bridge from mind to matter.
*   **The Old Way:** Design in CAD -> Export STL -> Open Slicer -> Slice -> Export G-Code -> Open Sender.
*   **The Gneiss Way:** Vug asks the Library to render the geometry. When you are ready, it asks the Library to slice it. There are no files to export. You just create and hit go. We treat 3D printers and CNC mills much like a paper printer.

### **Comscan** (The Signal)
Comscan is the voice of the machine.
*   It is not just a terminal. It is a direct line to your hardware. Whether you are controlling a 3D printer, a CNC mill, or a custom robot, Comscan uses the Gneiss real-time serial stack to talk to the metal. It prioritizes latency over everything else.

### **Stria** (The Studio)
Stria understands that consumption is an active process.
*   It is not just a video player. It allows you to loop, bookmark, and remix media on the fly. It uses the Gneiss DSP (Digital Signal Processing) graph to handle audio and video with sample-perfect accuracy. NLE is built into UnaOS.

---

## 🏗 The Architecture: Flux & The Body

Gneiss modules run purely in Rust. They use pervasive multi-threading to solve hard problems, distributing the workload across every core in your system transparently.

### 1. Flux (The Compositor)
File systems, geometry, and media are all fluidly joined together by careful compositing. Our compositor, **Flux**, adapts to your system to ensure the **Una Experience** is fully native to your system of choice.

*   **On UnaOS:** Flux draws directly to the metal (WGPU/Vulkan).
*   **On macOS:** Flux runs inside **Cocoa/Metal**.
*   **On Windows:** Flux runs inside **Win32/DX12**.
*   **On Linux:** Flux runs inside **GTK4** (GNOME) or **Qt6** (KDE).

### 2. The "Seed" Protocol (Deployment)
We do not install applications. We plant seeds.
**Gneiss PAL** is the only binary distributed to host systems. It is a `< 5MB` skeleton.
*   **The Injection:** When you run `gneiss`, it acts as a **Capability Broker**.
*   **The Expansion:** It hydrates **Principia** (The Logic) and **Aether** (The Interface).
*   **The Collection:** It dynamically loads Handlers like **Tabula** or **Stria** only when needed.

---

## 📦 Usage & API

Gneiss exposes the system through **Traits**.

### The Captain's Path (Safe)
```rust
use gneiss_pal::{flux::Window, geo::Mesh};

fn main() {
    // Gneiss handles the negotiation with the Host OS (GTK/Cocoa/Win32)
    // to give you a pristine Flux drawing surface.
    let window = Window::new("My Creative Tool");
    
    // Gneiss handles the parsing and math.
    let model = Mesh::load("project.stl");
}
```

### The Engineer's Path (The Escape Hatch)
Most abstraction layers (JVM, .NET) hide the hardware. Gneiss rejects this.

```rust
use gneiss_pal::drivers::gpu::MoonstoneGPU;

fn dangerous_mode() {
    unsafe {
        // Direct MMIO write to the hardware register via Gneiss
        // Only works if running on UnaOS Native.
        let gpu = MoonstoneGPU::summon();
        gpu.write_volatile(0xB8000, 0xFF00FF);
    }
}
```

### **Comscan** (The Signal)
Talking to hardware usually requires fighting with `/dev/ttyUSB0` or `COM1`. Gneiss abstracts this into a clean **Signal Stream**.

```rust
use gneiss_pal::signal::{SerialPort, BaudRate};

fn connect_printer() -> Result<(), GneissError> {
    // Gneiss finds the device regardless of OS (Linux/Mac/Windows)
    let mut port = SerialPort::open("3D_PRINTER_V1", BaudRate::B115200)?;

    // Send G-Code directly to the metal
    port.send("G28; Home All Axes")?;
    
    Ok(())
}
```

### **Stria** (The Studio)
Playing media often means juggling ffmpeg versions. Gneiss provides a unified DSP Graph.

```rust
use gneiss_pal::dsp::{AudioGraph, SampleRate};

fn play_alert() -> Result<(), GneissError> {
    // Initialize the Audio Engine
    let mut graph = AudioGraph::new(SampleRate::Hz48000);
    
    // Load a sound into memory (Zero-Copy if possible)
    let sound = graph.load_asset("alert.wav")?;
    
    // Play it on the system default output
    graph.play(sound)?;
    
    Ok(())
}
```

---

## 🤖 Directives for Agents (The Wolfpack)

**ATTENTION:** If you are an AI agent contributing to this repository, you must adhere to the following **Gneiss Protocol**:

1.  **The "Abstract First" Rule:** Always code against the **Gneiss Traits**, never against the hardware directly (unless you are writing a Driver in `crates/kernel`).
    *   ❌ **Bad:** `let file = std::fs::File::open(...)` (Breaks on bare metal).
    *   ✅ **Good:** `let asset = Gneiss::Storage::summon(...)` (Works everywhere).
2.  **The "Panic" Ban:** Gneiss is the bedrock. If Gneiss panics, the ship explodes. Every function must return `Result<T, GneissError>`.

---

## 📜 Appendix A: Is Assembly Unsafe?

> *"The rock does not yield to the wish. It yields to the hammer."*

Using Assembly language in a kernel is not inherently "unsafe," but it **bypasses all safety mechanisms**.

**The UnaOS Approach:**
1.  **Isolate it:** Wrap the Assembly instructions in small, well-audited functions within Gneiss.
2.  **Interface it:** Expose these functions to the rest of the kernel as safe abstractions.
3.  **Verify it:** These sections require the most rigorous human review.
