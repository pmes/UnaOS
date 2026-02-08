# 🔨 Aulë: The Monorepo Hammer

> *"Aulë is your power of creation."*

**Aulë** is the workspace manager for **UnaOS**. It is a heavy-duty tool designed to the power of a god over the complexity of managing repositories.

While Git is excellent for single repositories, it struggles with the scale of an OS composed of multiple interdependent shards (`UnaOS`, `Stria`, `Vug`, `Midden`). Aulë wraps Git in a layer of **Iron Logic**, ensuring that the entire ecosystem moves in lockstep.

---

## 🧱 The Philosophy

### 1. The Monolith (One Workspace)
Aulë treats the `UnaOS/` directory not as a collection of folders, but as a **Single Truth**.
*   **Traditional Git:** You have to `cd` into `kernel`, pull, `cd` into `bootloader`, pull, and hope they match.
*   **Aulë:** You run `Aulë sync`. It iterates through every shard defined in `Cargo.toml`, aligns their branches, and ensures the **Gneiss PAL** dependency is synchronized across the board.

### 2. The Snapshot (Global State)
Aulë introduces the concept of a **"Snap"**—a synchronized tag across multiple repositories.
*   When we release **Moonstone v0.1**, Aulë tags the Kernel, the PAL, and the Shell simultaneously.
*   It generates a "Manifest" that guarantees you can rebuild the exact state of the OS 10 years from now.

### 3. The Future: UnaFS Backed
Currently, Aulë acts as a "Supervisor" for Git.
**The Roadmap:** Aulë will eventually bypass `.git` folders entirely and interface directly with **UnaFS**.
*   **Database-Driven:** Version control becomes a metadata query. "Show me the Kernel as it existed on Tuesday."
*   **Backwards Compatible:** Aulë will still push/pull to standard Git remotes (GitHub/GitLab) for collaboration, but the local "Source of Truth" will be the **UnaFS Database**, not a loose collection of text files.

---

## ⚒️ Usage (The Strikes)

Aulë is built as part of the workspace.

```bash
# 1. SYNC (The Daily Driver)
# Pulls all shards, updates dependencies, and checks for "Dirty" states.
cargo run -p Aulë -- sync

# 2. STATUS (The God View)
# Shows the branch, commit hash, and "Crystal Color" of every shard.
cargo run -p Aulë -- status

# 3. SNAP (The Release)
# Creates a unified tag across the entire ecosystem.
cargo run -p Aulë -- snap "moonstone-v0.1"
```

---

## 🏗 Architecture

Aulë operates on the **"Iron/Diamond"** principle.

*   **Phase 1: Iron (Current)**
    *   Wraps `std::process::Command("git")`.
    *   Parses `stdout` to detect dirty states.
    *   Enforces `Cargo.toml` workspace member consistency.

*   **Phase 2: Diamond (Future - UnaFS)**
    *   Directly reads repository objects from the **UnaFS** database.
    *   Calculates Merkle Trees natively in Rust (no `libgit2` dependency).
    *   Provides "Time Travel" debugging for the Kernel.

---

## 🤖 Directives for Agents (The Wolfpack)

**ATTENTION:** If you are an AI agent (J1-J20) contributing to this repository, you must adhere to the following **Aulë Protocol**:

### 1. The "Clean Hands" Rule
**NEVER** commit directly to `master` or `main` inside a shard.
*   **Rule:** All changes must be on a Feature Branch (e.g., `feature/wolfpack-usb`).
*   **Enforcement:** Aulë will block a `sync` if it detects a detached HEAD or a commit on a protected branch without a Pull Request.

### 2. The "Atomic Strike"
When refactoring a shared library like **Gneiss PAL**:
1.  Update `gneiss_pal`.
2.  Update `unaos` (Kernel) to use the new version.
3.  Update `stria` (Media) to use the new version.
4.  **Aulë Snap**: Commit all three simultaneously. Do not leave the build in a broken intermediate state.

---

## 🔮 Roadmap

*   [ ] **Phase 1: The Wrapper** - Robust `sync` and `status` commands.
*   [ ] **Phase 2: The Enforcer** - Pre-commit hooks that run `cargo fmt` and `cargo test` across the whole workspace.
*   [ ] **Phase 3: The Vault** - Initial integration with **UnaFS** for local metadata tracking.

> *"Strike while the iron is hot. Commit while the code is green."*
