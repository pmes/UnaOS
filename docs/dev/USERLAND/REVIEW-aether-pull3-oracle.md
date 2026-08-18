# REVIEW — Aether pull 3 oracle results: FAIL (both shells), root causes attached

Gates were green, but Peter's oracle runs fail: GTK shows widgets and a permanently
white viewport for real sites; macOS shows a fully featureless white window. The
reviewer root-caused all three defects — the ENGINE IS NOT AT FAULT (probe-verified:
fixture HTML, example.com, and the error page all paint correctly through
`load_html` + `render_frame`). These are shell/API bugs. Fix exactly these:

## 1. GTK white viewport — RefCell borrow held across await (the bug)
`shell_gtk.rs`: `engine_ref.borrow_mut().load_url(&url).await` holds the engine
borrow for the entire network fetch. The 16 ms tick also calls `borrow_mut()`; the
first tick during a fetch = `BorrowMutError` panic, the load future dies, the page
never lands. Same defect in the `go_back()`/`go_forward()` handlers.
**Fix pattern (all three call sites):** never hold the borrow across an await —
fetch first with NO engine borrow, then apply synchronously:
```rust
let html = aether::net::fetch_document(&url).await;          // no borrow held
match html {
    Ok(h)  => engine.borrow_mut().load_html(&url, &h, true), // sync, brief borrow
    Err(e) => engine.borrow_mut().load_error_page(&url, &e), // sync
}
```
Engine side: add the small sync helpers this needs (`load_error_page`, and
history-aware variants for back/forward that return the target URL so the shell can
fetch it). `load_url(&mut self).await` as a public API is a trap — every RefCell/
single-threaded shell will hit this; deprecate it in favor of the split
fetch-then-apply pattern (the macOS proxy-event design already works this way).

## 2. macOS featureless window — URL row gated off by default
`shell_macos.rs`: the synthetic URL row draws only `if self.url_bar_active`, which
defaults to `false`, and nothing else is on screen — hence a bare white window.
**Fix:** the URL row is ALWAYS drawn (active state only changes its highlight);
draw the current URL text + a caret when focused; click in the top 30 px focuses it;
Enter loads. Verify an initial `request_redraw()` fires after window creation so the
first frame isn't blank, and blit respecting the engine's width (not the raw buffer
index math currently assuming equal strides).

## 3. Scheme-less input must work ("google.com" → loads)
Peter's requirement: typing `google.com` must just work. Add URL normalization IN
THE ENGINE (both shells benefit): if the input parses without a scheme, try
`https://<input>`; on connect failure fall back to `http://<input>`; keep
rejecting `file://`. Unit-test the normalizer offline (no network in tests).

## Gates for the fix pull (unchanged rules + one addition)
- Pasted gate output per phase commit; tree green (`cargo test -p aether`,
  `--features gtk` check on Linux, plain check owed on the Mac).
- NEW: an offline shell-logic test that fails on a borrow-across-await regression
  (e.g. a test driving load + tick on a single thread), so this class can't return.
- Oracle re-run (Peter): GTK and macOS both load `google.com` typed bare, show the
  page, scroll; macOS URL row visible from first frame.
