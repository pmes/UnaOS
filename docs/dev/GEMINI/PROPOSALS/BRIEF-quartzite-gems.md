# BRIEF — kill the per-platform browser.rs; build reusable quartzite gems (proposal first)

**Ruling (Peter, 2026-07-22):** `platforms/{gtk,macos,qt}/browser.rs` is WRONG design.
Three per-platform files hardcoding one app's chrome is not reusable — it is the
opposite. quartzite's value is **reusable native gems** (like `meter`, `widgets`,
`tetra`) that ANY vessel composes; a "browser" is not a quartzite concept, it is Aether
composing gems. This supersedes the earlier (my-approved, wrong) browser.rs direction
AND the Qt-build brief.

## The correct shape
1. **Remove `browser` from quartzite entirely** — the `pub mod browser` /
   `pub use platforms::*::browser`, and `platforms/{gtk,macos}/browser.rs` (qt never
   got one). No app-specific module lives in the GUI library.
2. **Add the missing REUSABLE gems to quartzite** — the generic primitives a chrome
   needs, each mapped to native widgets ONCE per platform, usable by any vessel:
   - a text-input gem (→ NSTextField / GtkEntry / QLineEdit),
   - a button gem (→ NSButton / GtkButton / QPushButton),
   - a pixel/canvas content-view gem that presents a client BGRA buffer
     (→ NSImageView / GtkDrawingArea / QQuickPaintedItem),
   - a row/column layout gem (→ NSStackView / GtkBox / QBoxLayout).
   Extend the existing `tetra`/`widgets` vocabulary rather than inventing a parallel
   one. These gems are the deliverable — reusable, not Aether-specific.
3. **Aether composes the gems in ONE platform-agnostic place** (the vessel): describe
   the chrome (row: back, fwd, url-input; below: content-view) once, wire it to bandy
   `SMessage` once. quartzite translates that description to native per platform via its
   existing spline/bootstrap machinery. No per-platform Aether code, no build.rs in the
   vessel.

## Why this also fixes the Qt build
Once chrome is generic gems composed by the vessel, there is no `shell_qt.rs` and no
`aether-shell/build.rs` — the cxx-qt-build wiring stays inside quartzite (where the Qt
backend and its cxx-qt deps already live). Qt "just works" like GTK/macOS because there
is nothing Aether-specific per platform to build.

## Proposal first
Submit `PROPOSALS/PROPOSAL-quartzite-gems.md` (STATUS: PROPOSED). It must show: the gem
API (the reusable primitives + how a vessel composes them), how Aether's chrome is
expressed with them in one place, and how each gem maps to the three native toolkits.
Do NOT keep any `browser.rs`. Reviewed before code.

## Gate
`cargo run -p aether-shell` on macOS (native window, real widgets, google.com renders +
title updates) AND `--features gtk` on Linux AND `--features qt` on KDE — same vessel
code, three native results, zero per-platform Aether files. Oracles are Peter's on each
box; don't claim a pass you didn't run there.
