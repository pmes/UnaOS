# 💎 Facet: The Prism

> *"Light is information. Color is code. We do not just look; we inspect."*

**Facet** is the high-performance raster graphics engine of **UnaOS**. It rejects the slow, bloated "Photo Viewers" of legacy systems that blur images to hide the pixels.

**Facet** embraces the pixel. It is a tool for precision inspection, texture manipulation, and instant visual feedback.

## 🌈 The Philosophy: The Raw Pixel

Modern image viewers lie to you. They apply smoothing, color correction, and compression artifacts. **Facet** shows you the raw buffer.

### 1. The Loupe (Inspection)
Facet is designed for the Game Developer and the Digital Artist.
*   **Infinite Zoom:** Zoom in until a single pixel fills the screen. See the exact RGBA values.
*   **The Grid:** Toggle a pixel grid to count spacing perfectly. No more guessing.
*   **Channel Splitting:** Isolate the Alpha channel instantly to check transparency masks. View Red, Green, and Blue independently to debug compression artifacts.

### 2. The Shader (Processing)
Facet is not just a viewer; it is a GPU-accelerated processor.
*   **WGPU Core:** All rendering and adjustments happen on the metal. 0ms latency on filters.
*   **Instant LUTs:** Apply Color Look-Up Tables (LUTs) to preview grading without altering the source file.
*   **Texture Ops:** Automatically detect Normal Maps and Roughness Maps. Visualize them on a 3D sphere with one keystroke (via **Vug** integration).

## ⚙️ The Mechanics

### The Buffer
Facet loads images directly into GPU memory. It bypasses the CPU bottleneck for massive 8K textures and RAW photos.
*   **Format Agnostic:** Reads PNG, JPG, QOI, KTX2, and DDS (DirectDraw Surface) natively.
*   **Sprite Sheet Slicing:** Automatically detects grid boundaries in sprite sheets for quick animation previews.

### The Crop
Non-destructive editing.
*   **Region of Interest:** Select a box, get the coordinates, export the slice.
*   **Batch Convert:** Select 100 images in **Matrix**, drag them to **Facet**, and convert them to optimized web formats instantly.

## 🛑 The Kill List
Facet replaces:
*   **macOS Preview / Windows Photos**
*   **Adobe Photoshop** (for cropping, resizing, and inspection)
*   **PureRef** (for reference boards)
*   **Texture Packers / Viewers**
