# 🔨 Aulë: The Workspace Smith

> *"The rock does not yield to the wish. It yields to the hammer."*

**Aulë** is the sovereign workspace manager for **UnaOS**. It is designed to wield the power of creation over the chaos of distributed version control.

While Git is excellent for single repositories, it fractures under the weight of an Operating System composed of multiple interdependent Shards (`UnaOS`, `Stria`, `Vug`, `Midden`). Aulë wraps the chaos of Git in a layer of **Iron Logic**, ensuring that the entire ecosystem moves in perfect, crystalline lockstep.

We want you to be you--Git can be run by you in Midden.

## 🧱 The Philosophy

### 1. The Monolith (One Truth)
Aulë treats the `UnaOS/` directory not as a loose collection of folders, but as a **Single Truth**.
*   **The Old Way:** You traverse directories, pulling and merging, hoping the Kernel matches the Bootloader.
*   **The Aulë Way:** You invoke the Smith. Aulë aligns every shard defined in the workspace, ensuring the **Gneiss PAL** dependency is mathematically synchronized across the entire nervous system.

### 2. The Snapshot (The Fossil Record)
Aulë introduces the concept of a **"Snap"**—an atomic crystallization of the entire OS state.
*   When we release **Moonstone v0.1**, Aulë does not just tag a commit. It captures the exact vibrational state of the Kernel, the PAL, and the Shell simultaneously.
*   It generates a **Manifest** that guarantees you can rebuild the exact state of the OS 10 years from now, regardless of how the internet changes.

### 3. The Future: UnaFS Native
Currently, Aulë acts as a "Supervisor" for Git.
**The Destiny:** Aulë will eventually bypass `.git` folders entirely and interface directly with **UnaFS**.
*   **Database-Driven:** Version control becomes a metadata query. "Show me the Kernel as it existed on Tuesday."
*   **Backwards Compatible:** Aulë will still push/pull to standard Git remotes (GitHub/GitLab) for collaboration, but the local "Source of Truth" will be the **UnaFS Database**, not a loose collection of text files.

## ⚒️ The Rites (Capabilities)

Aulë is not ran; it is invoked.

### 1. SYNC (The Alignment)
*   **Purpose:** To bring order to the local workspace.
*   **Action:** Iterates through the Shard list. Pulls upstream changes. Verifies that local modifications (The "Dirty" State) are safe. Re-links local paths to ensure the OS compiles as one unit.

### 2. STATUS (The God View)
*   **Purpose:** To see the layout of the land.
*   **Action:** Reports the branch, commit hash, and "Crystal Color" of every shard.
    *   **Green:** Clean, Synced, Ready.
    *   **Amber:** Local changes present.
    *   **Red:** Detached Head or Conflict.

### 3. SNAP (The Forging)
*   **Purpose:** To create history.
*   **Action:** Creates a unified tag across the entire ecosystem. It locks the version numbers in `Cargo.toml` and stamps the "Moonstone" seal onto the code.
