Midden: The System Archivist
"We do not pipe text. We compile intent."

Midden is a next-generation shell environment and system archivist designed for unaOS. It rejects the POSIX philosophy of spawning heavy, clumsy legacy binaries (grep, find, ls) in favor of Direct System Execution.

Instead of gluing black-box tools together, Midden compiles your intent into specialized Rust artifacts called Shards, executes them directly via the Gneiss PAL (Platform Abstraction Layer), and stores the result in a semantic Knowledge Pile.

🏗 Architecture
1. The Knowledge Pile (The Archive)
Midden does not treat the filesystem as a stranger. It maintains a persistent, indexed state of the system.

Traditional Shell: Runs ls -R to find a file, forgetting the result immediately.

Midden: Queries the internal Archive. If the Archive is stale, it updates the index via direct kernel calls, then answers the query instantly.

2. The Shard (The Execution Unit)
For complex tasks, Midden creates Shards.

Concept: A Shard is a transient, hyper-specialized Rust binary compiled on-the-fly.

Example: Instead of grep "impl" ./src | wc -l, Midden generates a minimal Rust struct that opens the directory, reads the bytes, and counts the matches using zero-copy memory mapping.

Benefit: Zero process-spawning overhead, type-safe data handling, and maximum speed.

3. Gneiss PAL (The Bedrock)
Midden interacts with the hardware through Gneiss PAL, a cross-platform abstraction that allows it to run on:

Host Mode: Fedora Rawhide (for development and build management).

Target Mode: unaOS (as the native shell 0).

🛠 Developer Information
Midden is currently in the "Princess" Phase (Pre-Alpha). It is delicate, strict, and requires precise handling.

Prerequisites
Rust Toolchain: nightly (Strictly enforced via rust-toolchain.toml).

Build Instructions
Midden is part of the unaOS workspace.

Bash
# Build Midden (Host Mode)
cargo build --package midden --release

# Run the Midden REPL
cargo run --package midden
🤖 Directives for Agents (Jules)
ATTENTION: If you are an AI agent (Jules) contributing to this repository, you must adhere to the following Shard Protocol:

1. The "Anti-Legacy" Rule
DO NOT use std::process::Command to invoke system tools like git, grep, or gcc unless absolutely unavoidable (e.g., the bootstrap compiler).

Incorrect: Spawning ls to list files.

Correct: Using std::fs::read_dir or Gneiss PAL primitives.

2. The "No Unwind" Constraint
Midden must be compatible with the no_std kernel environment of unaOS.

Avoid panic!. Use Result<T, E> for all control flow.

Do not pull in dependencies that require libc unless they are behind a feature = "std" flag.

3. The "Moonstone" Visual Standard
Visual Output: All terminal output should respect the unaOS visual identity.

Color: Moonstone Purple (Approx 0x2C003E background).

Feedback: Midden does not speak unless spoken to, or when reporting a [SHARD FAILURE].

🔮 Roadmap
[ ] Phase 1: The Indexer - Implement the file walker and "Knowledge Pile" database (Sledge integration).

[ ] Phase 2: The Compiler - Implement the on-the-fly rustc invocation to build simple Shards.

[ ] Phase 3: The Shell - Replace the user's login shell with Midden on the Host.

[ ] Phase 4: The Sovereign - Port Midden to run as PID 1 inside unaOS.

"Midden remembers."
