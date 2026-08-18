# PLAN — Aether pull 2 (for the Gemini session): corrections + A2-2 + A2-3 + A2-4

**A2-1 verdict from Peter's bench: PASSED** — the GTK shell runs on the Linux box and
renders a real site's bones. This pull takes the browser from "bones" to "usable",
across all three shells, in one run.

**Format note:** big pull — run all phases in sequence, commit per phase on
`UnaOS-gemini`, no per-phase review wait. Review once at the end.
**Standing gate per phase:** `cargo check -p aether -p aether-shell` (with the phase's
features) + `cargo test -p aether` green + REPORT section with honest oracle output.
No internet-dependent tests. Flag any out-of-lane touch (outside `handlers/aether/**`,
`vessels/aether-shell/**`) in REPORT before making it.

## Phase C — corrections (do first)
1. **Restore a real test suite** — deleting `yt/` removed every test; `cargo test` green
   is currently vacuous. Add engine-level tests on fixture HTML (no network): parse →
   layout → `render_frame` into a buffer; assert element positions and painted pixels
   for a small known page; storage path sanitization; form URL-encoding.
2. `handlers/aether/src/main.rs` must import everything from the `aether` lib — delete
   its duplicate `pub mod` declarations (everything currently compiles twice and types
   are duplicated).
3. Remove the mocked SurfaceBlit/`"url"` publish and the unconditional
   `mark_dirty()` 16 ms tick from `ignite()` — repaint only when `needs_repaint` is
   actually set by DOM/JS/layout changes.

## A2-2 — input, navigation, damage (the "usable" milestone; GTK shell)
4. Engine API: `handle_event(Event)` (mouse move/click/scroll, key, IME text, resize)
   and `tick()` (run JS jobs/timers) returning whether repaint is needed. Shells never
   reach into engine internals.
5. Link clicks navigate; back/forward history; URL bar updates on navigation; form
   fields take keyboard focus and text; submit works end-to-end.
6. Scrolling with damage tracking: repaint only invalidated regions; measure and report
   frame times in REPORT (target: smooth scroll on a typical page, no full-page
   recompute per frame).
7. Oracle (Peter, GNOME box): browse between pages of a real site by clicking links,
   type into a search box and submit, scroll smoothly, resize reflows.

## A2-3 — Qt shell (KDE)
8. Implement `shell_qt.rs` via cxx-qt (reuse quartzite's plumbing where it fits): same
   URL bar + surface + input forwarding, `--features qt`. The engine API from A2-2 is
   the contract — if Qt needs an API change, change the API and update GTK too; the
   shells must stay behaviorally identical.
9. Oracle (Peter, KDE): same script as (7), identical behavior GTK vs Qt.

## A2-4 — macOS shell
10. `shell_macos.rs`: winit window + softbuffer presentation + input mapping (incl.
    scroll momentum + Cmd-key conventions), built by target on macOS, no gtk/qt deps
    pulled in.
11. Oracle (Peter, Mac): same script as (7).

## Phase P — polish pass (fold into the above as you go, verify at end)
12. Title bar shows the page `<title>`; loading state visible (URL bar or spinner);
    errors render as a simple in-window error page, never a crash or a blank hang.
13. Every panic path in engine event handling audited: a malformed page must never
    take down the shell (catch at the engine boundary, render the error page).

## Explicitly OUT of this pull
- Media playback / Stria wiring (PlayMedia stays dormant), workers, WebCrypto, MSE.
- GPU-accelerated rendering (softpaint only; keep the surface API GPU-ready).
- Any UnaOS-native (bandy/vessel compositor) integration beyond keeping `ignite()`
  compiling — host shells are the target of this pull.

**End-of-pull deliverable:** REPORT section per phase with oracle output + measured
frame times, list of cross-lane touches (expected: none), and a 5-line "how Peter
tests each platform" crib.
