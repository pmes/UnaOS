# 🦅 Mach's Journal: The Native Bridge

## 2026-02-26 - [Workspace Target Collision & FFI Isolation]
**Learning:** Cargo workspaces do not inherently respect target-OS boundaries for binaries or FFI bindings. A blanket `--target x86_64-unknown-uefi` command forces host-native UI crates (like `quartzite`, which binds to `glib-sys`/AppKit) into the bare-metal compilation graph, causing immediate C-binding failures because `pkg-config` cannot resolve for `none` environments.
**Action:** Implemented strict target-gating in `quartzite` (`cfg(not(target_os = "none"))`) and utilized `default-members` in the root `Cargo.toml`. This physically prevents the host OS UI bridges from ever attempting to compile into the Ring 0 substrate.
