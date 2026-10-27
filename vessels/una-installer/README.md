# UnaOS Installer (`una-installer`)

The `UnaOS_Installer` vessel ([`docs/ROADMAP.md`](../../docs/ROADMAP.md) §1c, "The deliverable has a name"): what
people download. Two faces of one program: (a) install UnaOS itself onto a machine or card; (b) build, install and run
individual UnaOS vessels on the foreign host (macOS and Linux today, Windows someday), so a person adopts the native
suite one vessel at a time.

This README covers the **`vessels/una-installer`** vessel. It is not a handler and is not one of the 21 locked
handler names in [`docs/CODEX.md`](../../docs/CODEX.md); it is a source vessel (see the vessel definition in
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)). LEDGER SR413 (INSTALLERVESSEL);
the card face is SR414 (INSTALLERCARD).

## What it is

A vessel is an executable composed at startup from shared libraries and handlers. The installer owns no domain:

- **Build.** Aulë (CODEX §2 Forge) builds a vessel from a source checkout: `aule::Aule::forge_in` runs
  `cargo build -p <crate> [--features <face>] --release` in the checkout and streams its lines.
- **Card.** Amber Bytes (CODEX §2 Disks) writes UnaOS onto a card: the `card` module (INSTALLERCARD).
- **Window.** `libs/quartzite`: one tetra tree through `Backend::new_tetra_vessel` (the stria-view shape), evaluated
  on AppKit (macOS) and GTK (`--features gtk`). Two panes: **Install UnaOS** (`card::face()`) and **Install apps** (one
  row per vessel: Build, Install, Run).
- **Signals.** `libs/bandy`: the apps pane's buttons fire `SMessage::Input { target: "installer", .. }`; the card
  face's buttons and field fire `SMessage::ConsoleInput`. One serve task owns the host snapshot and the card state.
- **State.** `libs/gneiss_pal` `UnaPaths`: the log goes to `<UnaPaths::root()>/logs`. The installer keeps no record of
  its own: a vessel is installed when its binary is in the apps directory.

## What it is for

Rung 1 is the developer's loop, honestly: a checkout plus a toolchain. The loop it is built toward (Peter,
2026-10-27): the installer writes UnaOS to the SD card in the Mac or Linux box, the target machine (the Zenbook first)
boots from it, the kernel's flight recorder logs that boot to the card (BOOTLOGCARD, rmbp-ledger B549, on a UnaFS
card), the machine reboots into its host OS, and the person's own AI reads the card and analyses the flight. Owed, not
built: INSTALLER-LANDING (read the card's flight log back on the host, copy it to `docs/dev/evidence/<round>/`, hand
it to the AI through gneiss_pal's `ModelProvider`), INSTALLER-PROVIDER (the person's own AI, Claude or Gemini today
through gneiss_pal's provider seam, chosen in Settings, none hardwired), INSTALLER-CONTRIBUTE (share the landed evidence
and ledger rows back upstream), INSTALLER-BUGREPORT (a bug report from a landed flight).

## Invocation

```sh
cargo run -p una-installer --features macos            # macOS: the window
cargo run -p una-installer --features gtk              # Linux with the GTK4 development files: the window
cargo run -p una-installer                             # Linux without a feature: the headless face (census + list)
cargo run -p una-installer -- census                   # the host census line, then exit
cargo run -p una-installer -- install pulse            # build (when the toolchain is here), copy, report
cargo run -p una-installer -- run pulse                # spawn the installed binary
```

`--checkout DIR` names the UnaOS source tree; without it the installer walks up from the working directory and from
its own binary to the first directory elessar reads as UnaOS (`MEMORIA.md`).

## The catalog

| vessel | crate | macOS face | Linux face |
|---|---|---|---|
| Aether, the web browser | `aether-shell` | `--features macos` | `--features gtk` |
| Una, the code workspace | `una` | (AppKit by target_os) | `--features gtk` |
| Lumen, the companion | `lumen` | `--features macos` | `--features gtk` |
| Stria, the video editor | `stria-view` | `--features macos` | `--features gtk` |
| Facet, the image viewer | `facet-view` | (AppKit by target_os) | none |
| Pulse, the system monitor | `pulse` | (AppKit by target_os) | none |
| Phonolite, the tone generator | `phonolite` | (AppKit by target_os) | none |

The rows live in `src/catalog.rs`; the host test reads the root `Cargo.toml` and each vessel's manifest, so a row
naming a crate, directory or feature the tree lacks goes red.

## Install, rung 1

- The apps directory: macOS `~/Applications/<crate>`; Linux `~/.local/bin/<crate>` plus
  `~/.local/share/applications/<crate>.desktop`; Windows is refused (`why=windows-unbuilt`).
- The build needs rustup (the repo pins a dated nightly in `rust-toolchain.toml`) and the face's toolkit: the Xcode
  Command Line Tools on macOS (`xcode-select -p`), the GTK4 development files on Linux (`pkg-config --exists gtk4`).
  Missing either, the installer copies a release binary already in the checkout (`built=0 why=<name>`) or refuses by
  name.
- A downloadable bundle that needs no toolchain is owed (INSTALLER-APPBUNDLE), as is fetching a prebuilt vessel from a
  release (INSTALLER-DOWNLOAD).

## The wire

```text
[installer] host os=<macos|linux|windows> arch=<arch> rustup=1|0 toolkit=<appkit|gtk4|qt6|gtk4+qt6|none> apps_dir=<path|none> checkout=<path|none>
[installer] installed vessel=<crate> path=<dest> built=1|0 why=<-|no-rustup|no-xcode-clt|no-gtk4-dev>
[installer] refused vessel=<crate> why=<windows-unbuilt|no-home|no-face|no-checkout|no-rustup|no-xcode-clt|no-gtk4-dev|build-failed|build-spawn|copy-failed|spawn-failed|unknown-vessel>
[installer] run vessel=<crate> path=<dest> pid=<pid>        (or why=not-installed)
```

## Composed handlers

| handler | crate | what the installer uses |
|---|---|---|
| Aulë (Forge) | `handlers/aule` | `Aule::forge_in`: the build in the checkout, its lines, its exit status |
| Amber Bytes (Disks) | `handlers/amber_bytes`, `unaos/libs/sys/amber_core` | the card face (`card`, INSTALLERCARD) |

## Status

Read-checked; the host tests (`cargo test -p una-installer`) cover the catalog, the census and the plan. The windowed
faces' first build and run are Peter's, on his Mac and a Linux box with GTK4. Owed: INSTALLER-CARD (SR414),
INSTALLER-APPBUNDLE, INSTALLER-DOWNLOAD, INSTALLER-VEIN (SH-4's AI socket), INSTALLER-WINDOWS (not started),
INSTALLER-LABEL (Quartzite's tetra tree has no text node, so the window does not yet show the census or the verb
lines; they go to stdout and the log), and the four rows under "What it is for".

## See also

- [`docs/ROADMAP.md`](../../docs/ROADMAP.md) §1c (SH-3, SH-4, the deliverable).
- [`vessels/aether-shell/README.md`](../aether-shell/README.md) (running a vessel on a Mac).
- [`docs/dev/blueprint/handlers.md`](../../docs/dev/blueprint/handlers.md) §5 INSTALLERVESSEL.
