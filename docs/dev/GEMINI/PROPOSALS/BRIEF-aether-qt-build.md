# BRIEF — make the Aether Qt/KDE build work (proposal first)

Peter is building `cargo run -p aether-shell --features qt` and it fails:
`cannot find crate cxx_qt_build` in `vessels/aether-shell/build.rs`. Qt does not build.
Figure out the right fix and submit `PROPOSALS/PROPOSAL-aether-qt.md` (STATUS: PROPOSED)
BEFORE writing code.

## Facts to work from
- `build.rs` calls `cxx_qt_build::CxxQtBuilder`, but `cxx-qt-build` is NOT in
  `aether-shell`'s `[build-dependencies]`. That is the immediate compile error.
- quartzite already depends on the whole cxx-qt stack at **0.9.1** behind its `qt`
  feature (`cxx-qt`, `cxx-qt-lib`, `cxx-qt-build` — see `libs/quartzite/Cargo.toml`),
  and has a real Qt backend (`libs/quartzite/src/platforms/qt/`: `mod.rs`, `window.rs`,
  `main_window.cpp/.h`, `vein_bridge.rs`).
- BUT there is **no `platforms/qt/browser.rs`** — GTK and macOS each got a
  `browser.rs` in the pivot; Qt did not.
- Inconsistency to resolve: `aether-shell` still carries its own `build.rs` +
  `src/shell_qt.rs`, even though the pivot MOVED the GTK and macOS shells out of the
  vessel and into `quartzite/platforms/*/browser.rs`. GTK/macOS vessels have no
  per-platform shell file or build.rs. Qt is the odd one out.

## The question your proposal must answer
Should Qt follow the same shape as GTK/macOS — i.e. a new
`quartzite/platforms/qt/browser.rs` implementing `bootstrap_browser` (QQuickPaintedItem,
raw input → SMessage, SurfaceBlit → view, per the earlier approved Qt design), with the
cxx-qt-build wiring living in **quartzite** (which already has it) — and
`aether-shell/build.rs` + `src/shell_qt.rs` deleted so the vessel is backend-agnostic
like the other two? If not, justify why aether-shell needs its own build.rs when GTK and
macOS don't.

Whatever you propose, the outcome is: `cargo run -p aether-shell --features qt` COMPILES
and runs a native Qt window with the same wiring GTK has (URL enter → OpenDocument,
buttons → nav, SurfaceBlit → content view, title update).

## Gate
Proposal reviewed first. On approval + implementation: `cargo run -p aether-shell
--features qt` compiles and runs on Peter's KDE box (that is the oracle — do not claim a
pass you did not run there). Note in the proposal whether the Qt toolchain build has to
happen on Linux (it does — say so, don't fake a check).
