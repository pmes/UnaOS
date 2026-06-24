# J1 "Vanguard" Architectural Log

## 2026-03-07 - [CXX-Qt Hybrid Framework Ignition]

**Anomaly:**
During the initial port of Lumen to a hybrid Qt framework (Qt Widgets skeleton + QML reactive interior), the CXX-Qt 0.8 build pipeline and GNU static linker repeatedly panicked.
1. `cxx-qt-lib` primarily supports pure `QGuiApplication`/`QQmlApplicationEngine` applications out of the box, causing runtime `QWidget` aborts when attempting to initialize a `QMainWindow`.
2. The GNU static linker aggressively garbage collected the generated QML Rust plugin (`com.unaos.lumen`) because there were no explicit C++ reference markers, causing `module not found` errors at QML engine runtime.
3. Traditional Qt macros like `Q_IMPORT_PLUGIN` failed to resolve correctly due to `cxx_qt_build` generating specific initialization semantics without standardized plugin wrappers for static builds.

**Resolution:**
1. **Opaque Application Bridge:** Implemented an opaque `LumenQApp` C++ wrapper across the FFI border to strictly initialize a `QApplication`, completely bypassing the fatal `QGuiApplication` constraint.
2. **Explicit Init Binding:** Injected the auto-generated `extern "C" void cxx_qt_init_crate_quartzite()` strictly into the `main_window.cpp` constructor to legally compel the GNU linker to embed the Rust `cxx-qt-build` data payloads.
3. **Manual Type Registration:** To cleanly bypass fragile `.qmldir` QRC resolution in static linking environments, explicit `qmlRegisterType<LumenApp>(...)` directives were utilized to map the Rust QObjects to the internal `QQmlEngine` immediately prior to `setSource()`.
4. **Channel Threading:** Resolved disconnected UI updates by establishing a `OnceLock<CxxQtThread>` that captures the QObject execution context via QML `Component.onCompleted`, subsequently allowing the detached Tokio background reactor to safely dispatch `GuiUpdate` instructions across thread boundaries.

**Next Steps (Phase 2):**
1. **Complex List Models:** The `history` and `preflight` properties were temporarily suspended to isolate and verify the engine ignition. They must be re-integrated using robust list/model methodologies compatible with CXX-Qt 0.8 to facilitate complex text rendering.
2. **Widget Componentization:** Break the monolithic `main.qml` apart. Create reusable QML UI partials inside `libs/quartzite` to function as the "Lego parts bucket."
3. **Window Resizing & TeleHUD:** Flesh out the structural Qt Widget skeleton, linking the `NativeWindow` abstractions correctly so Lumen can manipulate window chrome state, and ensure the UI elegantly handles sliding splits.
