# 🧠 Midden: The System Archivist

> *"We do not pipe text. We compile intent."*

**Midden** is the context-aware shell environment for **UnaOS**. It rejects the POSIX philosophy of spawning heavy, clumsy legacy binaries (`grep`, `find`, `ls`) in favor of **Direct System Execution**.

Instead of long command line arguments gluing black-box tools together with string manipulation, Midden compiles your intent into specialized Rust artifacts called **Shards**, executes them directly via **Gneiss PAL** (Plexus Abstraction Layer), and stores the result in a semantic **Knowledge Pile**.

Turning off AI in UnaOS installs the full complement of POSIX tools if you have not already done so. We recommend skipping them. Default expert mode does not rely on the standard executables. Midden gives you the illusion you're calling them executing directly from your text.

---

## 🏗 Architecture

### 1. The Crystal (Visual Status)
Midden communicates the emotional state of the system via the **Crystal Indicator** in the prompt. It doesn't just wait for input; it reports health. For example:

*   🟢 **GREEN:** Stable. Last command successful (Exit Code 0). Git tree clean.
*   🟠 **AMBER:** Caution. Background jobs active, or working in a "Dirty" git state (Uncommitted changes tracked by **Vairë**).
*   🔴 **RED:** Critical. Last command failed, or system invariant violated.
*   🔵 **BLUE:** Una Mode. The AI Interface is active and listening.

### 2. The Knowledge Pile (The Archive)
Midden maintains a persistent, indexed state of the system.
*   **Traditional Shell:** Runs `ls -R` to find a file, forgetting the result immediately.
*   **Midden:** Queries the internal Archive. If the Archive is stale, it updates the index via direct kernel calls, then answers the query instantly.
*   **Context:** It remembers *why* you are in a directory. If you open a project, Midden loads the relevant **Elessar** context automatically.

### 3. The Shard (The Execution Unit)
For complex tasks, Midden creates **Shards**.
*   **Concept:** A Shard is a transient, hyper-specialized Rust binary compiled on-the-fly.
*   **Example:** Instead of `grep "impl" ./src | wc -l`, Midden generates a minimal Rust struct that opens the directory, reads the bytes, and counts the matches using zero-copy memory mapping.
*   **Benefit:** Zero process-spawning overhead, type-safe data handling, and maximum speed.

---

## 🛠 Developer Information

**Status:** "Princess" Phase (Pre-Alpha).
*Midden is delicate, strict, and currently requires precise handling.*

**Prerequisites:**
*   Rust Toolchain: `nightly` (Strictly enforced via `rust-toolchain.toml`)
*   **vairë:** Must be present in the workspace to handle version control status.

**Build & Run:**
Midden is part of the `UnaOS` workspace.
```bash
# Build Midden (Host Mode - Runs on Linux/macOS)
cargo build -p midden --release

# Run the Midden REPL
cargo run -p midden
```

---

## 🤖 Directives for Agents (The Wolfpack)

**ATTENTION:** If you are an AI agent (J1-J20) contributing to this repository, you must adhere to the following **Shard Protocol**:

### 1. The "Anti-Legacy" Rule
**DO NOT** use `std::process::Command` to invoke system tools like `git`, `grep`, or `gcc` unless absolutely unavoidable (e.g., the bootstrap compiler).
*   ❌ **Incorrect:** Spawning `ls` to list files.
*   ✅ **Correct:** Using `std::fs::read_dir` or **Gneiss PAL** primitives.

### 2. The "No Unwind" Constraint
Midden must be compatible with the `no_std` kernel environment of UnaOS.
*   Avoid `panic!`. Use `Result<T, E>` for all control flow.
*   Do not pull in dependencies that require `libc` unless they are behind a `feature = "std"` flag.

### 3. The "Moonstone" Visual Standard
Visual Output must respect the UnaOS identity.
*   **Color:** Moonstone Purple (Approx `0x2C003E` background).
*   **Feedback:** Midden does not speak unless spoken to, or when reporting a `[SHARD FAILURE]`.

---

## 🔮 Roadmap

*   [ ] **Phase 1: The Indexer** - Implement the file walker and "Knowledge Pile" database (vairë integration).
*   [ ] **Phase 2: The Crystal** - Implement the Status Line and Git/vairë state detection.
*   [ ] **Phase 3: The Compiler** - Implement the on-the-fly `rustc` invocation to build simple Shards.
*   [ ] **Phase 4: The Sovereign** - Port Midden to run as `PID 1` inside UnaOS (replacing the kernel init).

> *"Midden remembers."*
