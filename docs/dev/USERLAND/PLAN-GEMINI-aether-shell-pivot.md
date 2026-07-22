# PLAN — Aether shell pivot: onto quartzite, kill the hand-drawn chrome (for Gemini)

**Ruling (Peter, 2026-07-22):** the macOS shell's pixel-painted URL box is an
architecture violation — see [ARCHITECTURE.md §3 "Two presentation modes"]
(ARCHITECTURE.md). A vessel on a host presents the platform's REAL native widgets
via quartzite; it never hand-paints chrome. `vessels/aether-shell` with its bespoke
winit+softbuffer macOS backend reinvented quartzite badly — quartzite already ships
a real AppKit backend, GTK4, Qt, and `Backend::new_vessel(...)` for single-view
vessels (pulse uses it).

## The pivot
Aether's host shell routes through **quartzite**, not bespoke per-platform code.

1. **Delete the bespoke backends:** remove `shell_macos.rs` (winit+softbuffer, the
   font8x8 URL row, the hand-drawn caret) and the ad-hoc GTK/Qt window/URL-bar
   plumbing in `aether-shell`. The `winit`/`softbuffer`/`font8x8` deps go with them.
2. **Build on quartzite `new_vessel`:** the browser window is a quartzite vessel —
   a native window whose chrome (URL text field, back/forward/reload buttons) is
   quartzite's real host widgets on every backend (AppKit/GTK/Qt), and whose main
   content view displays the engine's rendered page. Study how `pulse` uses
   `Backend::new_vessel` and mirror it.
3. **Chrome ↔ engine wiring, via bandy `SMessage` (quartzite's contract):**
   - URL field submit → engine `load_html` (fetch-then-apply pattern, unchanged).
   - back/forward/reload buttons → the existing history API.
   - engine repaint → the content view updates from `engine.surface()`.
   - keyboard/mouse/scroll over the CONTENT view → engine `handle_event`. (Chrome
     input is the native widgets' own job now — no manual key handling for the URL
     field; the native text field does it.)
4. **Content view:** the rendered web page is legitimately pixels (it's foreign-soil
   web content), so the engine still renders into a buffer — that buffer is shown in
   a native image/canvas view provided by quartzite, NOT a raw softbuffer window.
   If quartzite lacks a "present a client-owned pixel buffer in a native view"
   affordance, STOP and report the gap — that's a quartzite arc, not a hand-paint
   excuse.

## Keep (already correct, do not lose)
Engine core: parse/layout/paint, `handle_event`, damage tracking, history,
scheme normalization (`google.com` → https/http), error-page synthesis, the
borrow-across-await fix, the offline test suite. The pivot is shell-only.

## Scheme + macOS typing
The engine-level URL normalizer stays (both shells inherit it). The macOS
"typing dropped" bug is MOOT after the pivot — a native `NSTextField` handles text
entry; delete the `Key::Character`/`Ime` hand-rolling along with the softbuffer shell.

## Gates
- `cargo test -p aether` green; `cargo check` for the quartzite-backed shell on
  Linux; macOS build check owed on the Mac (reviewer runs it).
- Remove the temporary `AETHER_DEBUG` GTK diagnostic once the white-viewport cause
  is understood (it may simply disappear with the native content view — report which).
- Oracle (Peter): on Mac AND GNOME, a NATIVE window — real title bar, real text
  field, real buttons — type `google.com`, see the page render in the content view,
  scroll. No pixel-painted chrome anywhere.

## Process
Proposal first (`PROPOSALS/PROPOSAL-aether-shell-pivot.md`, STATUS: PROPOSED). The
proposal must show how `new_vessel` builds the chrome and how the engine buffer
reaches a native content view on at least the macOS + GTK backends, reviewed before
code. Cross-lane note: this touches `libs/quartzite` if a content-buffer affordance
must be added — flag it explicitly.
