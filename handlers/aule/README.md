# Aulë (The Builder)

**Layer:** Layer 2 (Capability)
**Role:** Build System Wrapper & Task Runner
**Crate:** `handlers/aulë`

## 🔨 Overview

**Aulë** is the "Smith" of the UnaOS ecosystem. It is a specialized handler library that abstracts the complexity of compilation, testing, and task execution.

While **Tabula** edits the code and **Midden** runs the shell, **Aulë** is responsible for the **Build Loop**. It wraps the Rust toolchain (`cargo`), system compilers (`gcc`/`clang`), and task definitions into a unified programmatic interface.

## 🏗️ Architecture

Aulë sits at **Layer 2 (Handlers)** of the Trinity Architecture.

* **Input:** Receives build requests from **Vessels** (e.g., `apps/una`) or other Handlers.
* **Process:** Spawns and manages subprocesses (e.g., `cargo check --message-format=json`).
* **Output:** Streams structured diagnostics (errors, warnings) and build artifacts back to the `gneiss_pal` state.

## ⚙️ Capabilities

Aulë provides the following core services:

| Function | Description |
| --- | --- |
| **`check`** | Fast compilation for errors. Powers the "squiggles" in **Tabula**. |
| **`build`** | Full compilation (Debug/Release). Handles feature flags and targets. |
| **`test`** | Runs unit and integration tests with structured output. |
| **`clean`** | Manages `target/` directory hygiene and cache invalidation. |
| **`run`** | Executes the final binary with specific environment variables. |

## 🔌 Integration

**Used by `apps/una` (The IDE):**
Aulë is the engine behind the "Build" button and the "Problems" pane.

1. **Live Checking:** When a file is saved in **Tabula**, Aulë runs `cargo check` in the background.
2. **Diagnostics:** It parses the JSON output from Cargo and updates the editor's diagnostic collection.
3. **Task Running:** It executes pre-launch tasks (e.g., database migrations) defined in the project.

**Usage Example (Rust):**

```rust
use aulë::{Builder, Profile};

let builder = Builder::new("/path/to/project");
let result = builder.compile(Profile::Release).await?;

match result {
    Ok(artifact) => println!("Built at: {}", artifact.path),
    Err(diagnostics) => eprintln!("Build failed: {:?}", diagnostics),
}

```

## ⚠️ Status

**Experimental.**

* *Requirement:* Requires a valid Rust toolchain installed on the host OS (Fedora).
* *Constraint:* Currently optimized for Rust/Cargo projects. Support for C/C++ (via CMake/Meson) is planned.
* *Edition:* **Rust 2024**.
