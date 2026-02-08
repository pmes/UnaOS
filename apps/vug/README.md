# 💎 Vug: The Visual Cortex

> *"To the machine, a pixel is just data. To Vug, a pixel is depth."*

**Vug** (named after the small, crystal-lined cavities inside rocks) is the **Geometry Kernel** and **perception engine** of **UnaOS**.

It is far more than a 3D demo or a window manager. It is a **Spatial Interpreter** that bridges the gap between 2D imagery and 3D understanding.
Where other systems see a "JPEG," Vug sees a **Heightmap**. Where others see a "Window," Vug sees a **Plane**.

---

## 👁 The Philosophy

### 1. The Calibration (The Benchmark is the Setting)
When you first run UnaOS, Vug launches a "Demo"—a spinning, ray-traced crystal.
*   **The Myth:** This is just eye candy.
*   **The Reality:** This is a **System Calibration**. Vug measures the exact frame timing, memory bandwidth, and compute shader performance of your hardware.
*   **The Result:** It sets a global "Snappiness Profile" for the entire OS. If you are on a 10-year-old laptop, Vug disables the heavy blur shaders to ensure the UI remains instantly responsive (60fps locked).

### 2. The Perception (2D to 3D)
Vug rejects the flat world.
*   **Standard Viewer:** Opens a photo. It is a flat rectangle.
*   **Vug Viewer:** Opens a photo. It analyzes luminance, color, and edge contrast to generate a **Depth Map** in real-time.
    *   It allows you to tilt, pan, and "look around" a static image.
    *   It detects "Assemblies"—distinct objects within the image—and offers to break them apart into layers.

### 3. The Handoff (From Viewer to Editor)
Vug is the gateway to **Elessar**.
*   You are looking at a photo of a mechanical part in Vug.
*   You click "Extrude."
*   Vug instantly converts the depth map into a **Mesh**, hands the object to **Elessar**, and suddenly—without a loading screen—you are in a CAD environment, editing the geometry of what was just a flat picture moments ago.

---

## 🧱 The Architecture

### 1. The Ray Marcher (SDF)
Vug does not use triangles for its UI. It uses **Signed Distance Functions (SDFs)**.
*   **Mathematics:** Every button, window, and icon is defined by a mathematical formula, not a texture.
*   **Infinite Resolution:** You can zoom in on a Vug interface until you see the sub-pixels, and the curve will remain perfectly smooth.
*   **Tiny Footprint:** The entire UI code fits in the CPU L1 Cache.

### 2. The "Vug Protocol" (Graphics Shard)
Vug communicates with the **Gneiss PAL** to request the raw framebuffer.
*   **Direct Mode:** It bypasses standard compositors to write directly to video memory.
*   **Latency:** By owning the pixel pipeline, Vug guarantees "Photon-to-Motion" latency lower than any standard desktop compositor (Wayland/X11).

---

## 🛠 Developer Information

**Vug** is a high-performance graphics shard.

**Prerequisites:**
*   `vulkan` or `metal` support (Host Mode).
*   **UEFI GOP** (Target Mode - UnaOS).

**Build Targets:**
```bash
# Run the Vug Calibration Demo (Host)
cargo run -p vug --release

# Run the Unit Tests (Math Verification)
cargo test -p vug
```

---

## 🤖 Directives for Agents (The Wolfpack)

**ATTENTION:** If you are an AI agent (J1-J20) contributing to this repository, you must adhere to the following **Vug Protocol**:

### 1. The "Math First" Rule
**DO NOT** use large texture assets or heavy meshes.
*   **Rule:** If a shape can be defined by an equation (Sphere, Box, Torus), use the **SDF primitive**.
*   **Why:** We are building a system that fits on a floppy disk but looks like a AAA game. Math is lighter than textures.

### 2. The "60Hz" Mandate
Vug is the UI. If the UI drops a frame, the illusion breaks.
*   **Constraint:** All rendering logic must complete within **16.6ms**.
*   **Enforcement:** The build server will fail if any Vug shader exceeds the complexity budget.

---

## 🔮 Roadmap

*   [ ] **Phase 1: The Crystal** - Implement the SDF Ray Marcher and the Calibration Scene.
*   [ ] **Phase 2: The Viewer** - Implement the 2D-to-3D depth estimation shader for images.
*   [ ] **Phase 3: The Bridge** - Create the "Handoff Protocol" to send geometry from Vug to **Elessar**.

> *"The world is not flat. Neither is your data."*
