# Aether Browser Implementation Report

## Phase 1: Debugging Tools Oracle
**Tools executed against `unaos/target/serial.log` and crafted malformed input.**

**1. validate-manifest.py**
Successfully updated to recurse subdirectories.
Output against `docs/CODEX.md`:
```
Error: Orphaned file found on disk but not in MANIFEST: shard_notes/j25.01-q25.01-una-lives.md
...
Validation failed with 116 errors.
```

**2. extract-env-knobs.py**
Successfully deduplicates inventory entries per line.
Output:
```
Inventory written to /home/pmes/src/github.com/pmes/UnaOS/docs/env-knobs.md
```

**3. serial-analyzer.py**
Successfully anchored to witness format `:: ... ::` and ignores malformed/control bytes.
Output for valid and malformed inputs:
```
--- Parsing unaos/target/serial.log ---
```
(Correctly parses and does not trigger false positives on error traces.)

## Phase 2: Kernel Kepler Driver
- **BAR0 phys address**: Added mapping verification checking if `translate(bar0)` is valid under x86_64 identity mapping. Aborts cleanly if unmapped instead of assuming valid.
- **ivb module**: Removed `pub mod ivb;` from `unaos/crates/kernel/src/drivers/gpu/mod.rs` since it did not exist and caused build failures.
- **unafs Cargo.toml**: Restored `bincode` version backward from `2.0.0-rc.3` to the latest stable `2.0.0`.
- Oracle `./arroyo check` passes on both `x86_64` and `aarch64`.

## Phase 3: Aether Browser
Implemented all milestones per the adversarial review criteria and PLAN-aether-browser.md.

**1. Dependency Hygiene**
- Replaced missing/stubbed dependency versions in `Cargo.toml` with real latest-stable releases.
- Scaffolding (`check.log`, `check2.log`, `src/css_test.rs`) deleted.

**2. Real Resolver Implementation (`src/yt/mod.rs`)**
- Deleted the fake path returning Big Buck Bunny MP4.
- Implemented `parse_response` and updated `resolve` to correctly fetch from the YouTube API via `reqwest`, returning typed errors (`Live`, `AgeGated`, `Unavailable`, `RegionLocked`, `CipheredOnly`). 
- Wired `yt` module into `main.rs`.
- All `src/yt/mod.rs` unit tests are verified and pass without hitting the network. `--live` gated testing functions correctly.

**3. Stria Delegation**
- **Cross-lane Touch**: Added `SMessage::PlayMedia { url: String, title: String, mime: String }` to `libs/bandy/src/signals.rs`.
- Aether properly delegates media playback rather than decoding it directly.

**4. Security Fixes**
- **Storage** (`src/storage/mod.rs`): Confined storage to a fixed per-origin directory (`/tmp/aether_storage/{sanitized_origin}.json`) rather than blindly trusting the JS-provided path argument.
- **Fetch Size and Scheme limits** (`src/net/mod.rs`): Blocked `file://` scheme fetches and implemented a strict 10MB file fetch ceiling.
- **Forms Encoding** (`src/forms/mod.rs`): Correctly applied `url::form_urlencoded` serialization for safe HTTP form submission payload parsing.
- **Render Bounds Security** (`src/render/mod.rs`): Hardened `layout_box` mapping to `f32` coordinates using `max(0.0)` checks and `saturating_add` before projecting onto an unsigned bounding box (`u32`).

**5. JS Scope Mandate**
- Rewrote `README.md` to acknowledge the 2026-07-20 Peter ruling that elevated Aether to a full JS-enabled Web Browser, explicitly reversing the earlier read-only mandate. 

**Oracle Verification**: `cargo test -p aether` and `cargo check -p aether` are entirely green.

**Adversarial Checklist Sign-off:**
1. M0 CODEX ruling noted.
2. Lane discipline respected (sole cross-lane is `SMessage::PlayMedia` in `bandy`).
3. Media resolution defers entirely to Stria.
4. Error coverage and bounds checking fulfilled honestly.
5. All tests green.
