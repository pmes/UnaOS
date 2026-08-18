# PLAN-GEMINI-aether-pull3 Execution

This plan outlines the specific steps required to fulfill pull 3 requirements and resolve all remaining Phase A2 debt cleanly, adhering to the strict process rules provided.

## User Review Required
No major architectural decisions require review before proceeding, but per the prompt, each phase commit will contain its compilation output as proof.

## Proposed Changes

### Phase 1: Green Tree
Update test calls to use the correct `render_frame` signature containing `damage_rects`.

#### [MODIFY] [engine_tests.rs](file:///home/pmes/src/github.com/pmes/UnaOS/handlers/aether/src/engine_tests.rs)
- Provide a `&[(u32, u32, u32, u32)]` array containing `(0, 0, 100, 100)` when calling `render_frame`.
- Add a negative probe checking that pixel `(3, 3)` matches the default background (white).
- Gate check: Execute `cargo test -p aether` and paste the green summary in the Phase 1 commit message.

---

### Phase 2: macOS Compilation Fix
Restructure threading architecture in the winit backend to ensure `AetherEngine` stays strictly on the main UI thread.

#### [MODIFY] [shell_macos.rs](file:///home/pmes/src/github.com/pmes/UnaOS/vessels/aether-shell/src/shell_macos.rs)
- Remove `engine` from `std::thread::spawn` closures.
- Initialize `AetherApp` with a generic event loop channel proxy (`EventLoop::with_user_event()`).
- Emit a custom event (e.g., `enum AppEvent { Reload, GoBack, GoForward }`) from the tokio threads over the event loop proxy.
- Catch `WindowEvent::UserEvent(event)` in the winit handler and trigger `engine.load_url()`, `engine.go_back()`, and `engine.go_forward()` on the main thread safely.
- Gate check: Since Linux cannot compile `target_os = "macos"`, state "NOT COMPILED HERE — awaiting Mac check" in the Phase 2 commit message.

---

### Phase 3: A2-3 Qt Shell
Implement Qt Shell with raw event forwarding.

#### [MODIFY] [shell_qt.rs](file:///home/pmes/src/github.com/pmes/UnaOS/vessels/aether-shell/src/shell_qt.rs)
- Set up a standard `cxx_qt::bridge` mapping QML raw input directly to `AetherEngine` signals. 
- Due to cxx-qt 0.9 compatibility issues seen previously, we will write a streamlined, valid `shell_qt.rs` and `build.rs` bridging `QQuickPaintedItem` that accurately compiles against `cxx-qt 0.9.1` dependencies.
- Gate check: Execute `cargo check -p aether-shell --features qt` and paste output into the Phase 3 commit message.

#### [NEW] [build.rs](file:///home/pmes/src/github.com/pmes/UnaOS/vessels/aether-shell/build.rs)
- Use `cxx_qt_build` to compile `shell_qt.rs` for Qt.

---

### Phase 4: A2-2 Verification Debt
Finalize testing and reporting.

#### [MODIFY] [engine_tests.rs](file:///home/pmes/src/github.com/pmes/UnaOS/handlers/aether/src/engine_tests.rs)
- Add tests validating that clicking a link triggers navigation.
- Add tests validating that `KeyDown("BackSpace")` edits the focused form field.
- Add tests validating `KeyDown("Return")` attempts to submit.

#### [MODIFY] [REPORT.md](file:///home/pmes/src/github.com/pmes/UnaOS/handlers/aether/REPORT.md)
- Document the `ptr::copy` fixed-elements caveat directly next to the `render_frame` scrolling documentation.
- Document synthetic frame times for damage-tracked scrolling operations.
- Update phase statuses for pull 3 to `DONE` and macOS to `NOT COMPILED HERE`.

## Verification Plan
1. **Phase 1**: `cargo test -p aether` output captured and embedded.
2. **Phase 2**: Explicitly labeled `NOT COMPILED HERE` for external macOS verification.
3. **Phase 3**: `cargo check -p aether-shell --features qt` output captured and embedded.
4. **Phase 4**: `cargo test -p aether` and `REPORT.md` validation.
