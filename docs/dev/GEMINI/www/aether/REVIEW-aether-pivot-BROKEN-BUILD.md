# REVIEW — Aether shell pivot (commit fa50954e): DOES NOT COMPILE. Rejected.

The architecture is right. The code does not build. "Passed cargo check" in your
walkthrough is false for the target that matters — you cannot claim a macOS backend
works by checking it on Linux, and it does not work.

## The macOS quartzite backend fails to compile — 15 errors

Verified on a Mac (the only honest place to check cfg(target_os="macos") code):
`cargo check -p aether-shell` → 15 errors in the new AppKit code. Concrete list:

- `libs/quartzite/src/spline.rs:67` and `:79` — `cannot find type MacOSSpline in this
  scope`. You referenced a type you did not define/import for the browser path.
- `libs/quartzite/src/platforms/macos/browser.rs` — every AppKit widget construction is
  wrong for the objc2 version in this tree:
  - `NSStackView::class` / `NSTextField::class` / `NSButton::class` / `NSImageView::class`
    — `no associated function named 'class'`. That is not how objc2 in this repo
    constructs these. Look at how the EXISTING macOS backend
    (`platforms/macos/mod.rs`, and the `meter`/vessel views) builds a real widget and
    copy THAT pattern — do not invent an objc2 API.
  - `Retained<NSStackView>: Encode is not satisfied` (and NSTextField/NSButton/
    NSImageView) — you are passing a `Retained<...>` where an objc2 message expects an
    encodable ref. Again: match the existing backend's calling convention.
  - `no method named 'upcast' found for Retained<NSStackView>` — same root cause.

These are not one-line imports. This is a whole file written against an objc2 API that
does not exist here. The fix is to study `libs/quartzite/src/platforms/macos/mod.rs`
(and whatever `meter`/`new_vessel` already does that COMPILES) and build the browser
chrome the same way.

## Process rule you broke, again

You marked this done with "Passed cargo check -p aether-shell for GTK." GTK is not
macOS. quartzite compiles exactly ONE backend per target; checking GTK on Linux tells
you NOTHING about the AppKit code, which is the half a reviewer on a Mac will run first.
If you cannot compile a cfg-gated backend in your environment, you say
"NOT COMPILED — <target> owed" for that file and you do NOT write "passed." Claiming a
pass you did not run is the same failure as the Kepler wall-1 resubmit: asserting a
result the machine contradicts.

## What is actually fine (do not redo)

- Shape is correct: engine stays in `aether-shell`, chrome is native widgets, wiring is
  pure `SMessage` (`BrowserNav*`/`BrowserScroll`/`BrowserResize`/`BrowserKey`/
  `BrowserText` + `SurfaceBlit`), no engine reference passed into quartzite. The old
  softbuffer/font8x8 shells are deleted. Keep all of that.
- Engine tests still 7/7.

## To close this

1. Make `cargo check -p aether-shell` pass ON A MAC (macOS backend) AND on Linux (GTK
   backend). Both. Paste both outputs.
2. Fix `MacOSSpline` and rewrite `platforms/macos/browser.rs` against the objc2 pattern
   the existing macOS backend already uses.
3. Do not claim a pass for a target you did not build on. If the Mac is unavailable to
   you, say so and the reviewer builds it — but then it is NOT done until that build is
   green.
