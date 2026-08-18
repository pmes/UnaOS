# PLAN — Aether r2: a real browser with native windows (for the Gemini session)

**Ruling (Peter, 2026-07-21) — supersedes the YouTube-resolver direction and every
"read-only / no-JS" remnant:** Aether's goal is a real, normal, high-performance web
browser people want to use. The `yt/` resolver module and its milestone ladder are
**dropped** (delete the module; the typed-error parsing work is archived by git history,
not carried). The PNG output mechanism is **removed** — Aether renders into live native
windows, never image files.

**Working setup:** Peter is developing with you on a Linux box and will test the app as
it progresses on **GNOME, KDE, and macOS**. Branch: `UnaOS-gemini`. Lane:
`handlers/aether/**` + a new `vessels/aether-shell/**`; flag any other touch in your
report before making it.

## Architecture

Keep the split the repo already lives by (handlers headless, views to the user):

- **`handlers/aether` — the engine.** Fetch → DOM (html5ever) → style → layout (taffy)
  → paint, plus boa JS, forms, storage, event loop. No windowing, no PNG. Its output is
  a paint surface + input events in, over an in-process API (library crate) — bandy
  remains the UnaOS-native transport, but for host testing the shell links the engine
  directly.
- **`vessels/aether-shell` — one thin native shell per platform**, selected by cargo
  feature/target:
  - **GNOME: GTK4** (`gtk4-rs`) — render the paint surface into a `GtkDrawingArea`/
    snapshot; libs/quartzite already carries gtk4 bindings, reuse its plumbing where it
    fits rather than re-inventing.
  - **KDE: Qt 6** via `cxx-qt` (also already in quartzite) — same engine, QQuickItem or
    QWidget surface.
  - **macOS:** winit + softbuffer/wgpu window (native AppKit windowing without hand-rolled
    Objective-C); Peter tests on Mac directly.
- Shell responsibilities only: window, URL bar, back/forward/reload, input forwarding
  (mouse/keys/scroll/IME), clipboard. Everything else lives in the engine so the three
  shells stay thin and identical in behavior.

## Milestones (each ends: `cargo check` + `cargo test` green, REPORT.md updated,
commit on `UnaOS-gemini`)

### A2-0 — Excise
Delete `src/yt/`, the PNG/`render_to_image` output path, and any CLI plumbing that
wrote image files. Engine exposes `render_frame(&mut self, surface: &mut [u8], w, h)`
(or equivalent damage-based API) instead. Nothing may write image files.

### A2-1 — GTK shell first (Peter's Linux box, GNOME)
`vessels/aether-shell` with the GTK4 window: URL bar + engine surface + live reflow on
resize. Oracle: Peter opens a real site and scrolls it.

### A2-2 — Input + navigation
Clicks/links/history/forms wired through the shell into the engine; keyboard input into
form fields; scrolling with damage tracking (don't repaint the world per frame — this is
the "high-performance" bar; measure and report frame times).

### A2-3 — Qt shell (KDE)
Same engine, cxx-qt shell, feature-gated so one `cargo build` flag picks the shell.
Oracle: identical page behavior GTK vs Qt.

### A2-4 — macOS shell
winit-based shell building on macOS. Oracle: Peter runs it on the Mac.

### Ground rules (unchanged from r1, still binding)
- **No fabricated success.** A milestone that doesn't work is reported as not working.
- Latest stable deps, always; versions must exist on crates.io.
- Security posture from fix-r1 stands: no `file://` fetch, response caps, per-origin
  storage under a proper per-user data dir (fix the `/tmp/aether_storage` hardcode in
  A2-0 — use the platform data dir), percent-encoded form submission.
- The live-YouTube network test is deleted with the module; never commit tests that
  need the public internet to pass.
- Report blockers honestly in `handlers/aether/REPORT.md`; adversarial review at each
  milestone.
