# unaOS Developer's Guide

## 1. The Philosophy of the Hive

**To Jules (and all contributors):**
Your approach to code—clean, commented, and aesthetically pleasing—is not merely "good style"; it is an engineering necessity. In operating systems, obscurity is the parent of failure. Efficient code is the courtesy the present pays to the future.

**The BeOS Legacy:**
Your historical interest in operating systems such as BeOS is astute. BeOS understood that the CPU is a resource to be saturated, not coddled. The "Pervasive Multithreading" model was decades ahead of its time. We will adopt this. We will not build a monolithic wall; we will build a hive of hyper-efficient workers.

## 2. Coding Standards

### The "Aesthetic Necessity"
* **Comments are Mandatory:** Do not just explain *what* the code does (the syntax tells us that). Explain *why* it exists.
* **No "Magic Numbers":** Every constant must be named. If you are writing `0x45`, it better be defined as `USB_PACKET_HEADER_SIZE`.
* **Rust Style:** We follow `rustfmt` defaults, but with stricter rules on `unsafe` blocks. Every `unsafe` block must be accompanied by a comment explaining the invariant being upheld.

### The "Pervasive Threading" Pattern
* **Async by Default:** IO operations should never block the kernel thread.
* **Message Passing:** Prefer channels over shared memory (mutexes). We treat threads like micro-services within the kernel.

## 3. The Hybrid License Headers

We use a **Hybrid Licensing Model** to protect the OS while allowing ecosystem growth. You must apply the correct header to your file based on where it lives.

### A. Core System (Kernel, Bootloader, Drivers)
*Path: `/kernel`, `/boot`, `/drivers`*
* **License:** GNU GPLv3
* **Header:**
    ```rust
    /*
        unaOS Core System
        Copyright (C) 2026  The unaOS Contributors

        This program is free software: you can redistribute it and/or modify
        it under the terms of the GNU General Public License as published by
        the Free Software Foundation, either version 3 of the License, or
        (at your option) any later version.

        This program is distributed in the hope that it will be useful,
        but WITHOUT ANY WARRANTY; without even the implied warranty of
        MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
        GNU General Public License for more details.
    */
    ```

### B. Compatibility Layer (User Libraries, Translation Layers)
*Path: `/libs`, `/compat`, `/api`*
* **License:** GNU LGPLv3
* **Header:**
    ```rust
    /*
        unaOS Compatibility Layer
        Copyright (C) 2026  The unaOS Contributors

        This library is free software; you can redistribute it and/or
        modify it under the terms of the GNU Lesser General Public
        License as published by the Free Software Foundation; either
        version 3 of the License, or (at your option) any later version.

        This library is distributed in the hope that it will be useful,
        but WITHOUT ANY WARRANTY; without even the implied warranty of
        MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
        GNU Lesser General Public License for more details.
    */
    ```

## 4. The Clean Room Protocol (CRITICAL)

If you are implementing a feature that mimics proprietary behavior (e.g., loading Windows executables or macOS drivers), you **MUST** identify your role.

* **Group A (White Box):** You read the documentation/reverse-engineer the proprietary binary. You write **Specs**, not code.
* **Group B (Black Box):** You read the **Specs** from Group A. You write **Code**. You never look at the proprietary source.

*Violating this protocol endangers the entire project. When in doubt, ask.*

## 5. The Visual Singularity: "Refraction" Boot 👁️

**Objective:** Zero-Flicker, Infinite-Resolution Boot Sequence.
**Method:** Ray Marching / Signed Distance Fields (SDF).

### The Philosophy
Standard OS boots are "static" (loading a bitmap). unaOS is "dynamic" (simulating a reality). We do not display the logo; we **materialize** it.

### The Sequence
1.  **T-Minus 0 (Post-BIOS):** The Bootloader initializes a high-res GOP Framebuffer (Graphics Output Protocol).
2.  **The Beam:** A calculated ray of light traverses the void.
3.  **The Hit:** The ray intersects the mathematical definition of the "Stria Crystal" (an octahedron with internal noise).
4.  **The Refraction:** Using Snell's Law ($n_1 \sin \theta_1 = n_2 \sin \theta_2$), the background light bends through the crystal, splitting into spectral colors.
5.  **The Handover:** The kernel inherits this framebuffer state. The "Crystal" smoothly morphs into the user's login avatar or dashboard icon.

### Technical Constraints
* **No Drivers:** Must run on CPU or basic UEFI GOP.
* **Technique:** Sphere Tracing (SDF).
* **Code Size:** < 4KB (Math is lighter than textures).


# developer's guide

To Jules: Your approach to code—clean, commented, and aesthetically pleasing—is not merely "good style"; it is an engineering necessity. In operating systems, obscurity is the parent of failure. Efficient code is the courtesy the present pays to the future.

Your historical interest in Operating systems such as BeOS is astute. BeOS understood that the CPU is a resource to be saturated, not coddled. The "Pervasive Multithreading" model was decades ahead of its time. We will adopt this. We will not build a monolithic wall; we will build a hive of hyper-efficient workers.

/*
    unaOS Compatibility Layer
    Copyright (C) 2026  The unaOS Contributors

    This library is free software; you can redistribute it and/or
    modify it under the terms of the GNU Lesser General Public
    License as published by the Free Software Foundation...
    [Rest of standard LGPL header]
*/
