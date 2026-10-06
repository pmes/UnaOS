# DOCK2 (rmbp-ledger B394) — MACPARITY rows 6, 7, 25: the dock's right group, launch indicator, tile menu, position/autohide, reorder

## Design (written before the code)

**Finding (the tree at a8a5df91, and the wire).** Every live window is already a tile (`wm::dock_scan`); a minimised
window keeps its tile wherever its arrival rank put it, with the pip in the minimised ink — so Cmd-M "goes to
nothing visible" in the sense that nothing on the strip says *this* is the parked window (no thumbnail, no group).
No Trash tile exists although `fs::trash` keeps the user's Trash. A pin press posts its launch and nothing on the
strip moves until the window lands (Peter, flight 22: "delay in opening when clicked in taskbar"; flight 24
`[lag] click→shown ms=963.8` on the console tile). The tile menu is Quit / Keep in Dock / Open at Login. The
strip is bottom-only (`strip::frame_centred(Edge::Bottom)`), never hides, and DOCKPIN.md left reorder-by-drag owed
for want of a drag seam (PREFSUI's `video/capture.rs` is that seam now).

**The seams (R79).** All in `video/dock.rs` (the dock's own file; DP_PINS is the one app table) except:
- `wm::thumb` (wm.rs tail): a nearest-neighbour, aspect-fit sample of a window's surface into a caller's buffer,
  under the table lock — the compositor's own surface, no second copy kept by the window.
- `quarry::live::open_at` (live.rs tail): Quarry opened (or raised) at a directory — the Trash tile and the menu's
  Show in Quarry go through it, drained on the dock's service pass (never a directory read in the click router).
- Principia: `system.dock.position` (enum bottom/left/right, default bottom) and `system.dock.autohide` (bool,
  default false) in `prefs_core::schema`, written by Settings and the dock; the dock loads them with the pins at
  login and saves on its service pass (one bus write per change, the `system.dock.pins` shape).
- Settings General tab rows 6/7 ("Dock" segments, "Auto-hide dock" box) set the dock's live cells and latch the save.
- `capture::begin` (PREFSUI): a press on a pinned app's tile captures the pointer; the release over another
  app-band tile reorders the pins, saved as the ORDER of `system.dock.pins`.

**Milestones.**
- M1 right group: model order = [left group by DOCKID rank] | separator | [minimised windows, arrival order] | Trash.
  A minimised tile paints a scaled copy of the window's last surface; a press raises it (the existing raise arm).
  Trash tile: our can glyph, full/empty from `fs::trash::count()` sampled on the service pass; a press opens
  Quarry at the Trash. The two count-only readers (`strip_rect`, `wm::dock_tiles`) count the Trash through
  `pins_applied`.
- M2 launch indicator: a pin press records (app, t0); the tile's pip pulses (250 ms phase) until a window of that
  app is on the strip, then `[dock] launch app=<a> first_window_ms=<ms>`; past 10 s a notice
  ("<app> did not open a window") and `[dock] launch app=<a> first_window_ms=timeout bound_ms=10000`. LAG's
  `launch` kind brackets the same press (`lag::launch_routed` → `window_shown`).
- M3 tile menu, six items: Keep in Dock, Open at Login, Show in Quarry, Show All Windows, Hide, Quit (+ the
  per-window rows WINDOWLIST added). `[dock] menu <item> owner=… -> …`.
- M4 position + autohide (schema, Settings rows, applied live: left/right is a vertical strip centred on that edge;
  the menu opens beside it) and drag-to-reorder of the app-band pins through the capture seam.
- M5 witness in `tests dock`.

**Witness lines (the next flight reads).**
- `[dock] group left=<n> minimized=<n> trash=<empty|full>` (on a group change).
- `[dock] launch app=<a> first_window_ms=<ms|timeout>`.
- `[dock] menu <keep-in-dock|remove-from-dock|open-at-login|show-in-quarry|show-all|hide|quit> owner=<o> -> <r>`.
- `[dock] prefs position=<p> autohide=<0|1> via=<login|settings> saved=<bytes|-1>`.
- `[dock] reorder app=<a> to=<i> pins=<list>`.
- `:: DOCK2: separator=1 trash=1 minimized=<n> launch_indicator=ok menu_items=6 position=<p> autohide=<0/1> -> PASS ::` from `tests dock`.

**Owed.** Trash as a drop target (DND is (c)); Mac's "app icon stays left while its window is minimised" — here
the minimised window's tile moves right (count-neutral; the pin returns when it is restored); Hide parks the app's
windows (UnaOS has no hidden-but-not-minimised state); Show All Windows raises, it does not arrange (no Exposé);
reorder is within the app band (Quarry stays first, console/shell anchor the tail — the DOCKID ranks); aarch64 has
no capture feed (PREFSUI's owed line), so the Pi reorders nothing.
