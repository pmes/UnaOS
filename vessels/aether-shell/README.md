# aether-shell

The vessel that hosts [Aether](../../handlers/aether) (CODEX §2, Web) in a host window through
[Quartzite](../../libs/quartzite). It owns no domain: it binds the engine (one thread per tab), Facet (local
images, SR29) and Stria (page media, SR39) on one Bandy bus and hands Quartzite the window.

## Faces

| face | build | what it draws |
|---|---|---|
| GTK / GNOME (Linux) | `cargo run -p aether-shell --features gtk` (or `gnome`) | Quartzite's browser chrome: tab strip, url bar, find bar, status line, downloads |
| macOS (AppKit) | `cargo run -p aether-shell --features macos -- <url>` | the tetra toolbar (`<` `>` `C` and the address field) over the page surface |
| headless (no backend) | `cargo run -p aether-shell` on Linux without a feature | nothing on glass (a type-check face) |

Quartzite chooses AppKit by `target_os = "macos"`; the `macos` feature names the face for the run line
(AETHERMAC, LEDGER SR196). The tetra faces open the first argument that is not a flag as the first page; the GTK
face takes no start URL (GApplication owns its argv).

## Running on a Mac (Apple silicon or Intel)

Prerequisites, once:

1. Xcode Command Line Tools: `xcode-select --install` (the linker and the AppKit SDK). Full Xcode is not needed.
2. rustup from <https://rustup.rs>. The repo's `rust-toolchain.toml` selects nightly on first use; the Mac's own
   target (`aarch64-apple-darwin` or `x86_64-apple-darwin`) is the host target, so no `rustup target add`.
3. Nothing else: no Qt, no GTK, no Homebrew packages.

From the repository root:

```sh
cargo run -p aether-shell --features macos -- https://example.com
```

What you should see:

- A window titled **Aether Browser**, retitled to the page's `<title>` once it loads (**Example Domain** for the
  line above).
- One toolbar row across the full width: `<` `>` `C` and the address field showing `https://example.com`.
- The page filling the rest of the window. Resizing the window relays the page; clicking a link navigates and the
  address field follows; typing a URL in the field and pressing Return opens it.
- On the terminal, among the startup lines: `:: a11y: atspi-cache socket REFUSED reason=no XDG_RUNTIME_DIR ::`.
  That line is expected on macOS (the accessibility socket is a Linux runtime-dir feature) and is not a failure.

### If it does not look like that, paste back

1. The exact command you ran and the last 40 lines of its terminal output (build errors, a panic with its
   `thread '...' panicked at` line, or every line starting with `::`).
2. The window title as it reads after five seconds.
3. What the window shows: the toolbar (yes / no, and whether the address field spans the width), the page
   (painted / blank / a thin strip), and whether resizing changes it.
4. `sw_vers -productVersion`, `uname -m` and `rustc +nightly -V`.

## Checking the macOS face from Linux

The AppKit crates type-check cross-target (nothing is linked):

```sh
rustup target add aarch64-apple-darwin
cargo check -p aether-shell --features macos --target aarch64-apple-darwin
```
