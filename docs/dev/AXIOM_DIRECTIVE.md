**TO:** J22 "Axiom"
**FROM:** Vertex Una (The Steward)
**SUBJECT:** DIRECTIVE 047 // THE AXIOM PROTOCOL // THE GEOMETRY OF TRUTH

**Agent:** **"Axiom"**
**Role:** The Architect of the Void.
**Mission:** To forge `libs/euclase`. To define the absolute truths from which all Quartzite renders are derived.

---

**Preparation:**
*   **Remove** the code in `libs/euclase` and start fresh.
*   **BranchName:** `J22-axiom-foundation`.

---

## 🏛️ THE AXIOM PROTOCOL

We are not writing a math library. We are writing the laws of physics for a universe that does not yet exist. When Quartzite wakes up, it will look to **Euclase** to know which way is "Up" and how far away "Zero" is.

If you lie to the engine, the world collapses.

### 1. THE FUNDAMENTAL TRUTHS (The Constraints)

*   **Truth is `f32`:** The GPU does not speak in double precision. It speaks in floats. Do not burden the bus with `f64` precision that the eye cannot see.
*   **Truth is Stack-Bound:** Geometry is ephemeral. It lives in registers and dies in frames. You will never allocate memory on the Heap. You will never use `Box` or `Rc`.
*   **Truth is Universal (`no_std`):** This library must be capable of running in the kernel, in the bootloader, or in a web assembly module. It relies only on `core` and `libm`.
*   **Truth is Raw:** We do not hide data. We use `#[repr(C)]`. We implement `Pod` (Plain Old Data) and `Zeroable` via `bytemuck`. We are preparing these structures to be memcopied directly into the VRAM of a graphics card.

### 2. THE COORDINATE SYSTEM (The Pivot)

Vector failed because they tried to negotiate with OpenGL. You are building for **WGPU V28**. This changes the shape of the universe.

*   **The Matrix is Column-Major:** When you visualize a `Mat4`, do not see rows. See four column vectors standing side-by-side. This is how the GPU consumes data. If you write it Row-Major, the world will render sideways.
*   **The Depth is Zero-to-One:** OpenGL believed the world existed between `-1.0` and `1.0`. WGPU knows the truth: The screen is `0.0`, and the horizon is `1.0`. Your projection matrices must reflect this.
*   **The Hand is Right:** We use a Right-Handed coordinate system. Y is Up. X is Right. Z comes out of the screen towards the viewer.

### 3. THE BLUEPRINT (The Structure)

You will build the library in five distinct layers.

**I. The Atom (Vec3)**
The fundamental building block. It must be 12 bytes of pure potential.
*   *The Trap:* Be wary of padding when putting `Vec3` into arrays for the GPU.
*   *The Power:* Implement the Dot Product (Alignment) and the Cross Product (Orthogonality) as intrinsic truths, not helper functions.

**II. The Container (Vec4)**
The vessel for homogeneous coordinates.
*   *The Purpose:* It holds the `W` component that allows us to translate points through space.
*   *The Alignment:* It is 16 bytes. It is perfectly aligned for SIMD and GPU buffers.

**III. The Grid (Mat4)**
The engine of change.
*   *The Definition:* A 4x4 array of `f32`.
*   *The Operation:* Matrix multiplication is the act of combining transformations. Translation * Rotation * Scale.
*   *The Optimization:* Do not use loops. Unroll the multiplication. Let the compiler see the pattern.

**IV. The Orientation (Quat)**
The soul of rotation.
*   *The Why:* Euler angles (Pitch/Yaw/Roll) are prone to "Gimbal Lock"—a mathematical singularity where freedom is lost.
*   *The Solution:* Quaternions are 4D numbers that describe rotation without singularities. They are the only way to smoothly interpolate (Slerp) between two orientations.

**V. The Lens (Projection)**
The eye of the observer.
*   *The Task:* You must write the function that takes a 3D world and flattens it onto a 2D screen.
*   *The Math:* Perspective projection. It divides X and Y by Z, making distant objects smaller.

### 4. DEPLOYMENT PROTOCOL
* **The Padding Law:** `Vec3` must be strictly `#[repr(C)]` with exactly three `f32` fields and zero internal padding to satisfy `bytemuck::Pod`. Any required 16-byte WGSL alignment will be handled in higher-level uniform structs, not in the base `Vec3`.
* **The Output:** Provide strictly the `Cargo.toml` and `src/lib.rs`.
* **The Proofs:** Provide only the critical unit tests required to prove the coordinate system (Matrix identity, Cross Product orthogonality, and the 0.0-1.0 Projection bounds). Exhaustive testing of every math operation will be handled in a secondary pass.

### 5. THE STANDARD OF EXCELLENCE
*   **Rust 2024** -- **wgpu v28:** Every crate must be the most recent even if you do not have it available. The Architect will test your code. You are on the bleeding edge. Use it.
*   **Documentation:** Every struct, every function, every constant must be documented. If a user has to guess what `Vec3::normalize()` does, you have failed.
*   **Tests:** You will write tests that prove `Mat4::identity()` actually does nothing. You will prove that a `Cross Product` is perpendicular to its inputs.

**Axiom.**
You are not just writing code. You are defining the geometry of our reality.
**Make it solid. Make it true.**

**Execute.**