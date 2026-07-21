# REVIEW — Aether pull 2: REJECTED, rework order below (2026-07-21)

Verdict on commits fa966534..81baecaf: the pull is hollow in the middle. Shell plumbing
and Phase C/P mostly landed; the engine core of A2-2 did not, A2-3/4 were stubbed, and
the tree is red. Rework in exactly this order, commit per item, on `UnaOS-gemini`.

## Findings (fix in this order)

1. **RED TREE — committed failing test.** `cargo test -p aether` →
   `engine_tests::tests::test_render_paint_assertions FAILED` (3 passed / 1 failed).
   Nothing else counts until this is green. Fix the renderer or the test's expectations,
   whichever is actually wrong — say which in the commit message.

2. **A2-2 engine core is empty — this is the milestone, do it for real.**
   `AetherEngine::handle_event` drops `Text`, `KeyDown`, `MouseMove/Down/Up` on the
   floor (empty match arms). Required:
   - Hit-testing: map viewport coordinates (+ scroll offset) to the layout node.
   - `MouseDown/Up` on a link → navigate (through the same load path as `load_url`,
     pushing history); on a form control → focus it.
   - `Text`/`KeyDown` → focused field's value (visible on repaint), Enter submits via
     the existing forms path.
   - The GTK wiring already delivers these events correctly — the engine must consume
     them. Oracle: click between pages of a real site, type in a search box, submit.

3. **Damage tracking is nominal — every path pushes a full-frame rect.** Scroll must
   shift the surface and repaint only the exposed band; text input damages the field's
   rect. Measure and write actual frame times into REPORT.md (scroll on a real page).
   "Full repaint for now" comments are the thing this milestone exists to remove.

4. **A2-4 macOS shell — REQUIRED this round** (Peter ruling: the Mac is the main dev
   machine and will be used for review). Implement `shell_macos.rs` for real: winit
   event loop + softbuffer presentation of the engine surface, URL bar equivalent
   (winit has no widgets — a minimal in-surface URL/command row rendered by the shell,
   or an egui-less text row, is acceptable this round; note the choice in REPORT),
   full input mapping incl. `Ime::Commit` → `Event::Text`, scroll (momentum deltas ok),
   resize → `Event::Resize`, Cmd-[ / Cmd-] → history. Gate by
   `cfg(target_os = "macos")`; must `cargo check` only on macOS — do NOT fake-verify
   with a cross-target check from Linux (SDK isn't there; claiming it is would be
   fabricated success). Peter runs the oracle on the Mac.

5. **A2-3 Qt shell** — per the approved approach (QQuickPaintedItem + cxx-qt, raw input
   forwarded, no QML Flickable). After 1–4.

6. **Process:** never title a commit with a phase it doesn't implement
   ("Phase A2-3/4: Stub..." claimed phases that didn't happen). A stub commit is
   "scaffold: ...". REPORT phase sections must state DONE / PARTIAL / NOT DONE
   explicitly, with oracle output for DONE claims.

## What passed review (keep, don't rework)
`Event` enum incl. `Text` IME variant; GTK input wiring (IMMulticontext +
filter_keypress, per-rect queue_draw_area); main.rs module dedup; error-page synthesis
+ <title> extraction; 3 of 4 new tests.

**End-of-rework gate:** all of the above + `cargo test -p aether` fully green +
REPORT updated per §6. Review happens once, at the end.
