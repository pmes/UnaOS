STATUS: APPROVED (2026-07-22, reviewer) — as written; https→http fallback on connect failure only.

# Aether Pull 3 Oracle Fixes Implementation Plan

This plan outlines the architecture fixes needed to address the three oracle test failures (GTK white viewport, macOS featureless window, and scheme-less input normalization).

## User Review Required

> [!IMPORTANT]
> - I will add `font8x8` as a macOS-specific dependency in `vessels/aether-shell/Cargo.toml` to support text rendering in `softbuffer` for the URL row.
> - The async `AetherEngine::load_url` will be deprecated to explicitly prevent "borrow held across await" panics in single-threaded shells, forcing shells to fetch via `net::fetch_document` *before* mutably borrowing the engine.

## Proposed Changes

### 1. Engine Core & Sync Helpers
**File: `handlers/aether/src/lib.rs`**
- [MODIFY] Add `pub fn load_error_page(&mut self, url: &str, error: &str)` helper.
- [MODIFY] Mark `pub async fn load_url` as `#[deprecated]` with a note advising the split fetch-then-apply pattern.
- [MODIFY] Create `test_shell_borrow_panic_prevention` inside `engine_tests.rs` (using a `LocalSet` to prove that the engine tick can run safely while a network request is pending).

### 2. Scheme-less URL Normalizer
**File: `handlers/aether/src/net/mod.rs`**
- [MODIFY] Add `pub fn normalize_url(input: &str) -> Vec<String>` which parses input.
  - Rejects `file://`.
  - Maps domain inputs to `vec!["https://<input>", "http://<input>"]`.
- [MODIFY] Update `fetch_document` to iterate over normalized URLs, falling back to HTTP *only* on connection failure.
- [NEW] Add offline unit tests for `normalize_url` covering fallback array sequences.

### 3. macOS Shell Blit & URL Bar
**File: `vessels/aether-shell/Cargo.toml`**
- [MODIFY] Add `font8x8 = "0.3"` under `[target.'cfg(target_os = "macos")'.dependencies]`.

**File: `vessels/aether-shell/src/shell_macos.rs`**
- [MODIFY] Update `WindowEvent::RedrawRequested`:
  - Enforce bounds checking via `w_w` (window width) and `e_w` (engine width) to resolve strides properly, rather than assuming equality.
  - Paint a 30px top bar unconditionally.
  - Utilize `font8x8::BASIC_FONTS` to blit the text of `self.current_url`.
  - Draw a caret if `self.url_bar_active` is true.
- [MODIFY] Issue `window.request_redraw()` immediately after `window` initialization to prevent the first frame from appearing blank.

## Verification Plan

### Automated Tests
- Run `cargo test -p aether` to verify the offline normalizer tests and the `test_shell_borrow_panic_prevention` task.
- Run `cargo check -p aether-shell --features gtk` and `--target x86_64-apple-darwin` to verify API surface integration.

### Manual Verification
- Reviewer checks GTK and macOS offline functionality (typing `google.com` leading to correct resolution via normalizer).
