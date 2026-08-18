# REVIEW — Aether oracle #2: mac typing root-caused; GTK white needs one instrumented run

Oracle results (Peter, 2026-07-22): macOS shows the URL row + caret but typing does
nothing; GTK loads (title changes to the site name) but the viewport stays white.
Reviewer findings — the ENGINE is again exonerated: headless probes render
google.com (480k non-white px), example.com, and Wikipedia correctly at 800×600.
Both failures are shell-side.

## 1. macOS — typing is dropped (CONFIRMED root cause, fix exactly this)
`shell_macos.rs` routes plain characters to `Key::Character(c)`, whose branch handles
ONLY `[` and `]`. Regular keystrokes are never appended to `current_url`; text entry
relies on `Ime::Commit`, which macOS does not fire for plain ASCII typing. The caret
blinks, every key is dropped, no URL can ever be submitted — the white page below is
simply "nothing was ever loaded."
Fixes:
- In `Key::Character(c)`: when `url_bar_active`, append `c` to `current_url` +
  `request_redraw`; when not active, forward as `Event::Text`.
- History shortcuts must require the Cmd modifier (check `event` modifiers /
  `SUPER`): bare `[`/`]` currently navigate history and would also swallow those
  characters in URLs.
- Keep `Ime::Commit` handling as-is (real IME input).
- While there: clean up the stream-of-consciousness comment block at the Enter
  handler (lines ~238-249) — decision comments stay, deliberation goes.

## 2. GTK — white viewport despite successful load (needs one diagnostic run)
Static review clears the obvious suspects: sizes match via connect_resize, stride is
w*4, alpha byte is 255, BGRA order matches Cairo ARgb32-LE, damage flow delivers a
full-frame rect on load, and the reviewer's headless probe confirms the engine
surface contains the rendered page for these exact sites. The failure is between
`eng.surface()` and the screen, and it cannot be reproduced on the review Mac.
Add a temporary diagnostic (behind `AETHER_DEBUG=1` env):
- In the draw func: log `w,h`, engine `width,height`, the damage count consumed by
  this draw, and a non-white pixel count of `eng.surface()` AFTER `render_frame()`.
- Dump the engine surface to `/tmp/aether-frame.ppm` once after the first
  post-load draw.
Peter runs GTK once with `AETHER_DEBUG=1`; the log + PPM decide whether the bug is
(a) engine surface white in-process (env/feature divergence), (b) surface non-white
but Cairo paint drops it (surface lifetime/mark_dirty), or (c) draw func not firing
post-load (queue_draw on the wrong widget instance). Fix follows the evidence;
remove the diagnostic in the same pull.

## Gates
Unchanged: pasted gate output per commit; `cargo test -p aether` green;
`--features gtk` check on Linux; Mac compile check owed at review (reviewer runs it).
Oracle re-run: type `google.com` on both shells → page visible, scroll works.
