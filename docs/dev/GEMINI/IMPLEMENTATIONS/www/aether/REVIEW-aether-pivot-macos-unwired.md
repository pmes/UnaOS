# REVIEW — Aether pivot runtime: macOS backend is a static stub (unwired)

Native chrome renders correctly on macOS (window, `<` `>` buttons, real `NSTextField`
with the typed URL) — the objc2 widget construction is right. But the window is blank
and the title never changes because **`platforms/macos/browser.rs` wires NOTHING.**
`_tx` and `_rx` are unused (they appear only in the signature). The GTK backend wires
all of this; the macOS backend built the widgets and stopped.

## What macOS must do — mirror what GTK already does

GTK's `bootstrap_browser` (working reference) does all of this; port it to AppKit:

1. **URL field Enter → `SMessage::OpenDocument`.** GTK: `url_bar.connect_activate(|e|
   tx.fire(OpenDocument{url}))`. AppKit: give the `NSTextField` a target-action (or
   delegate `controlTextDidEndEditing:` / the field's action on Enter) that fires
   `OpenDocument` on `tx`. Without this, typing + Enter does nothing → no fetch → blank.
2. **Buttons → nav.** `<`/`>` target-action → fire `BrowserNavBack`/`BrowserNavForward`
   on `tx` (GTK: `connect_clicked`).
3. **Consume `SurfaceBlit` on `rx` → update the `NSImageView` + window title.** GTK
   spawns a task that does `while let Ok(msg) = rx.recv().await { … update DrawingArea }`.
   AppKit equivalent: the blit arrives on a background thread but **UI mutation must be
   on the main thread** — marshal to the main queue (`dispatch_async` to main, or a
   main-thread timer that drains `rx`) and then: build an `NSImage` from the BGRA
   `pixels` and `setImage:` on the content `NSImageView`; set the window title from the
   blit's `url` field (the engine puts `engine.title` there). This is the piece that
   makes content appear and the title update — both currently missing for the same
   reason.

objc2 target-action needs a declared delegate/target class with the action method — use
the same pattern the existing macOS backend (`platforms/macos/mod.rs` and the workspace
delegates like `SidebarDelegate`) already uses. Do not invent an API.

## Not bugs — do not "fix" these
- The engine loop (`aether-shell/main.rs`) is correct: background thread, `load_html`
  sets `needs_repaint`, the 16 ms `tick()` fires the `SurfaceBlit`. macOS just never
  subscribes. (Minor robustness: you may fire an immediate blit right after `load_html`
  in the OpenDocument arm so content isn't gated on the next tick — optional.)
- GTK backend is fully wired (activate/clicked/scroll/resize/text/key → SMessage, and a
  SurfaceBlit consumer). Leave it.

## Gate
`cargo run -p aether-shell` **on a Mac**: type `google.com`, press Enter → page renders
in the content view AND the title becomes the page's `<title>`. That is the oracle you
could not previously reach because there was no wiring at all. Paste nothing as "passed"
until it runs on the Mac.
