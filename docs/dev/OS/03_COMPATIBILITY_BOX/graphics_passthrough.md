# Graphics Passthrough: The "Glass Wall"

## 1. The Risk
Direct GPU access is dangerous. A malicious shader can hang the GPU or read video memory from other apps.

## 2. The Solution: Virtualized Graphics (VirtIO-GPU)
* **The Proxy:** The app sees a "Generic High-Performance GPU" (e.g., a fake NVIDIA card).
* **The Translation:**
    * App sends: **DirectX 11** commands.
    * Layer converts to: **Vulkan** or **WGPU** (WebGPU for Native).
    * Host executes: Safe, validated Vulkan commands on the real hardware.

## 3. Shader Validation
Before any shader from a foreign app is executed, it passes through a **SPIR-V Validator**.
* We sanitize the code to ensure it cannot access memory outside its own texture buffers.
* This allows high-performance gaming while maintaining the "Air Gap" security.
