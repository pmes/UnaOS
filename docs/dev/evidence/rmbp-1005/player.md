# PLAYER (rmbp-ledger B419, MACPARITY row 30) — a window with transport controls for every sound the system claims

## Design (written before the code; cut from d27ec897)

**Finding (the wire, flights 24/25, and the code).** Quarry's double-click on a sound is
`[quarry] open PLAY path=… type=audio/… -> play (latched for the service tick)` (`quarry/openers.rs`, the `play` arm):
`hda::play::request_open` — headless, no window, the only stop is `play stop` at the serial door. The PCM path
(`drivers/hda_play.rs`) has `start`/`feed`/`finish`/`stop` and two producers (the WAV pump, the `play-dec` task since
MP3HANG/DECJOBHANG); it has no pause, no seek and no position read. `audio_core` (the shared decoder) has NO seek and
no seek table; `demux_core::Demuxer::seek` exists but is the VIDEO container's (keyframe seek over MP4/WebM sample
tables) and `audio_core::mp4` does not route through it — so a coded seek here is `method=decode-skip` (re-open, decode
and discard up to the target frame: exact, costs decode time) and the wire says `table=none`. WAV seeks by byte
arithmetic (`method=pcm-exact`). No `player` exists in the tree (searched). ATTRCOLUMNS (`media:duration_ms`,
`media:codec`) is on its branch, not merged: the window reads them through `MountTable::get_attr` and falls back to
the decoder's own `Info` (frames/rate, codec), and says which (`src=attrs|decoder`).

**Seam.** `video/player.rs` — `//! CHARTER: Kernel — wm` (a kernel window app, the fileview shape: one window, latched
open, chained from `quarry::live`'s service/press/key passes, the WINID holder registry for closes the WM makes).
The PLAY stays the HDA driver's: a short tail block in `hda_play.rs` adds `pause`, `seek_to`, `position` — the
decode path is untouched except that a paused consumer is not an orphan and a seek's skip count discards decoded
frames before the downmix. Volume is `status::set_volume` (the Settings slider's and the F10–F12 keys' one model,
the amp BEZEL reads back). The scrubber and the volume slider DRAG through PREFSUI's `capture` seam. Media keys:
`status::volkey_usage` (the EHCI decoder's key-edge seam) hands F7/F8/F9 (usages 0x40..0x42) to `player::media_key`
(atomics only); the player's pass acts and arms BEZEL's new play/pause kinds. The window's icon is APPRES's:
a `player` block (`unaos/res/player`, packed by `una-res pack`) in `appres::BUILTIN`. No knob: rides `wc` +
`hda-tone` (both in the metal line); without `hda-tone` the opener stays headless-refused as today.

**Milestones.**
- M1 `hda_play.rs` tail: `pause(on)` (RUN cleared/set, amp ramp, the decoder waits instead of orphaning),
  `seek_to(ms)` (WAV pcm-exact; coded decode-skip via a superseding `play-dec` job), `position()` (ms played from the
  ring's completed entries + LPIB), `now_playing()`. Wire: `[play] pause on=<0|1> run_bit=<0|1>`,
  `[play] seek to_ms=<n> landed_ms=<n> method=<pcm-exact|decode-skip> table=<pcm|none>`.
- M2 `video/player.rs`: the window (icon, file name, info line, play/pause, scrubber + elapsed/remaining, mute glyph,
  volume slider), one player (a second open replaces the first), close stops the play (close box, Cmd-W/menu via the
  WINID holder). Quarry's `play` opener opens the Player. `tests play` keeps its door path (the TQ queue) unchanged.
- M3 media keys F7/F8/F9 → the open player (previous = to 0, play/pause, next = to the end), BEZEL play/pause glyphs.
- M4 the info line from `media:duration_ms`/`media:codec` (attrs) else the decoder's count; `tests player` (R80: typed).

**Witness (the next flight reads).**
```
[player] open win=<n> path=<p> codec=<c> duration_ms=<n> src=<attrs|decoder> vol=<n>/16
[play] pause on=1 run_bit=0      [play] pause on=0 run_bit=1
[play] seek to_ms=<n> landed_ms=<n> method=<pcm-exact|decode-skip> table=<pcm|none>
[player] key media=<prev|playpause|next> -> <action>      [bezel] show kind=<play|pause> …
[player] closed win=<n> by=<closebox|wm|replace|fixture> stops=1
:: PLAYER: open=ok transport=ok seek=ok volume=shared close_stops=1 -> PASS ::   (tests player, TEST.WAV)
```

**Owed.** Video stays a `none` opener (SR26, Stria's). A coded seek decodes from the start (audio_core has no seek
table; a FLAC SEEKTABLE / Xing TOC seek is the shared core's to add). F7/F8/F9 from an xHCI keyboard on x86 (only
the EHCI decoder calls `volkey_usage` there). aarch64: the window opens, no audio path (`hda-tone` is x86).
