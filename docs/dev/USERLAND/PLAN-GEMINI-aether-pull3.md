# PLAN — Aether pull 3 (for the Gemini session): red-tree fixes + macOS + Qt

**Context:** the pull-2 rework was REJECTED (2026-07-21) on the same rule as the round
before: work was committed without running its own gates. The A2-2 engine work looks
substantive but cannot be assessed while the tree is red. This pull starts with a
zero-tolerance rule: **run the gate, paste its output into the commit message, for
every phase.** A phase commit without pasted gate output fails review automatically.

## Phase 1 — make the tree green (nothing else counts first)
1. `cargo test -p aether` currently does not COMPILE: `render_frame` gained the
   `damage_rects` parameter but `engine_tests.rs` still calls the old signature.
   Update the tests, run them, paste the green summary into the commit message.
2. While there: assert the negative paint probe too ((3,3) = body background), per the
   earlier review — a one-pixel paint test can pass by accident.

## Phase 2 — macOS shell that actually compiles (review machine = Peter's Mac)
3. `shell_macos.rs:167` moves `Rc<RefCell<AetherEngine>>` into `std::thread::spawn` —
   not `Send`, does not compile. Restructure, don't cast:
   - Engine stays on the main/UI thread, single-threaded, no `Arc<Mutex>`.
   - Network loads run on the tokio runtime; completed results (HTML string or error)
     come back via a channel or `winit::EventLoopProxy` user event; the event loop
     hands them to the engine.
   - This same pattern is what GTK already does with `spawn_local` — mirror it.
4. You cannot compile this on the Linux box (cfg-gated to macOS, no SDK) — so the
   phase's gate is different: state "NOT COMPILED HERE — awaiting Mac check" in the
   commit message, and Peter (or the reviewer) runs `cargo check -p aether-shell` on
   the Mac before the phase is called DONE. Do NOT title it "implementation complete".
5. Keep the pull-2 macOS requirements: winit event loop, softbuffer present of the
   engine surface, shell-drawn URL row (top 30 px), `Ime::Commit` → `Event::Text`,
   scroll/resize mapping, Cmd-[ / Cmd-] history.

## Phase 3 — A2-3 Qt shell (final open milestone from pull 2)
6. QQuickPaintedItem + cxx-qt per the approved design: raw mouse/wheel/key/text
   forwarded to `handle_event`, no QML Flickable, `--features qt`. Gate:
   `cargo check -p aether-shell --features qt` on the Linux box, output pasted.
7. Oracle (Peter, KDE): same browse/type/submit/scroll script as GTK, behavior
   identical.

## Phase 4 — A2-2 verification debt
8. With the tree green, the A2-2 claims from the rework need their evidence: frame
   times for damage-tracked scroll on a real page (GTK, Linux box) written into
   REPORT.md, and the fixed-elements caveat noted next to the ptr::copy scroll path.
9. Engine tests covering the new input paths: hit-test → link navigation on a fixture
   DOM; focus + Text/Backspace editing a field; Enter submitting the form (offline,
   fixture HTML only).

## Process rules (now standing for every Aether pull)
- Gate output pasted per phase commit; no pasted output = automatic reject.
- Never title a commit with a phase/milestone it does not fully contain.
- REPORT sections declare DONE / PARTIAL / NOT DONE / NOT COMPILED HERE.

**End of pull:** tree green on Linux (`cargo test -p aether`, `cargo check` gtk + qt);
macOS check owed on the Mac; REPORT complete. Review once, at the end; the GTK/KDE/Mac
oracle runs are Peter's.
