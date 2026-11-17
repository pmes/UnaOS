# Local backlog — the Stria suite, open-ended

The local seat never waits for the cloud seat to cut. When a slot frees, take the TOP unstarted entry, give it the
next id from the LOCAL BLOCK (SR850-SR999 are reserved for local; the cloud seat allots from SR840 upward and skips
this block), write its arc file from the entry (the pack-2 files are the template: ARC / CHARTER / GOAL / M1 / M2 /
M3 / OWED / FILES / SEAM / EXCEPTION), launch an Opus executor, measure its design, fold-test, push, tell the cloud
seat. Mark the entry `[SRnnn started]` / `[SRnnn pushed <sha>]` here and push this file with the branch. Run as many
at once as the machine carries; the seams named per entry are the only ordering. Every entry is measure-first: the
executor re-measures each anchor and the local seat rules on the design.

Row prefix for every entry: `- NLE5b (SR204 owed)` (handlers.md L104, edited IN PLACE) unless the entry names another.
Shared seams across the whole backlog: `VERBS` in nle_face.rs (append at the END of the one line, bump the count; the
seat composes the union), `* UNA <WORD>` EDL comment lines (one word per arc, own write line after `* UNA MARK`, own
parse arm), `Timeline` / `Track` / `Clip` / `Source` literals in timeline_core lib.rs (a new field = every literal gains
one line: merge the landed sibling before committing), view.rs `paint_in`'s optional decor lookup (SR827's: extend it,
never add a second parameter), the undo road (whole-timeline snapshot: name it once, reuse).

## A. Editing core (timeline_core lib.rs + nle_face.rs)
1. STRIARIPPLETRIM — trim-in / trim-out with ripple (the following clips move by the trimmed amount) as a `trim-ripple` modifier on the existing trim drag (measure SR737's trim op; ONE op variant through the undo road); EDL unchanged.
2. STRIAROLLEDIT — a roll edit on a cut (one drag moves the out of A and the in of B together; total length unchanged); the cut hit region (measure `Layout::hit` for an edge hit); verbs `roll-` / `roll+` by one frame.
3. STRIAINSERTOVERWRITE — `insert` / `overwrite` of a source range at the playhead on the selected track (three-point editing: measure whether a source viewer exists; if not, the source range is the clip under the playhead's src range: name the rule).
4. STRIAMULTISELECT — shift-click adds to the selection, a marquee drag selects clips crossing the rectangle (`Layout::crossing` :110 EXISTS: wire it); every clip op applies to the whole selection (move / delete / gain).
5. STRIACOPYPASTE — `copy` / `cut` / `paste` of the selection at the playhead (an in-face clipboard `Vec<Clip>`; paste on the selected track; overlaps refused by `validate`).
6. STRIANUDGE — `,` / `.` move the selection by one frame, shift by ten (through move_clip + undo); the message row reads the new `at`.
7. STRIACLIPLINK — audio and video clips from one source move together: a `group: u32` id on Clip (measure first; EDL `* UNA GROUP`).
8. STRIATRACKADDREMOVE — `track+ video|audio` / `track-` on the face (Timeline tracks Vec; EDL writes tracks by order already: measure).
9. STRIATRACKREORDER — drag a track header up / down (needs SR837's badge column or a header column: name).
10. STRIAGAPCLOSE — `close-gaps` removes every gap on the selected track (ripple); `gap <n>` inserts n frames of gap at the playhead.
11. STRIASLIPVIEW — while slipping (SR737), the preview shows the clip's new first and last frame side by side (two `preview` calls into one split still).
12. STRIAZOOMTOFIT — `zoom-fit` fits the whole timeline in the window; `zoom-sel` fits the selection (Layout.zoom + the view origin: measure `Face.view`).

## B. Audio (nle.rs mix + nle_face.rs meters)
13. STRIAAUDIOCROSSFADE — an audio transition (equal-power crossfade) at an audio cut: `Transition` on audio tracks (today video only per the `Track.transitions` doc); `mix_run` applies the curve; EDL `* UNA XFADE` EXISTS (edl.rs :219: measure what it carries today).
14. STRIAPAN — per-clip pan (`pan_cq8: i8`, -128..=127) applied in `mix_into` (constant power); verb `pan <-1..1>`; EDL `* UNA PAN`.
15. STRIATRACKGAIN — per-track gain (`Track.gain_cdb`) multiplied into every clip's gain in `mix_run`; verb `track-gain`; EDL `* UNA TGAIN`.
16. STRIAMETERSCLIPHOLD — the meters hold the peak for 2 s and show a red clip indicator that stays until clicked (`paint_meters` :1350; `MeterLevel`).
17. STRIAAUDIOSCRUB — scrubbing the ruler plays the audio under the playhead (a one-frame PCM burst per scrub step through the existing play road: measure the Player / audio sink).
18. STRIANORMALIZE — `normalize [<dBFS>]` sets the selected clip's gain so its peak (SR827's Peaks table: REUSE) hits the target (default -1 dBFS).
19. STRIALOUDNESSREADOUT — integrated loudness (ITU-R BS.1770 K-weighting + gating) for the export range on the message row (`loudness` verb); ONE pure fn in audio_core or timeline_core (name which).

## C. Video (nle.rs render + export)
20. STRIACROP — per-clip crop (l / t / r / b in 1/1000 of the frame) applied in the render step before the size policy; verb `crop`; EDL `* UNA CROP`.
21. STRIAPOSITIONSCALE — per-clip position / scale (picture-in-picture): the render blits the scaled source into the frame at (x, y); verbs `pip` / `scale`; EDL `* UNA XFORM`.
22. STRIAOPACITY — per-clip opacity for an upper video track over a lower one (measure `schedule`'s track priority and `vmix` first); EDL `* UNA ALPHA`.
23. [SR869 started] STRIACOLORLIFT — per-clip brightness / contrast / saturation (three i16 in 1/1000) as ONE per-pixel pass in the render step (RGBA); verb `color <b> <c> <s>`; EDL `* UNA COLOR`.
24. STRIAFREEZEFRAME — `freeze` splits at the playhead and inserts a 2-s hold of the frame (a rate-0 clip once SR829 is in, else a generated still source like SR828: name which).
25. STRIATRANSITIONLENGTH — drag a transition's edge to change its duration (`Layout::transition_rect` :402 EXISTS; the op through undo).
26. STRIAEXPORTPRESETS — `export --preset web|archive|proxy` as named bundles of the existing knobs (nle_export.rs table; the face's export verb accepts the word; stria-view lists them).
27. STRIAEXPORTQUEUE — queue several export jobs (ranges x presets) and run them in order off the face's thread (the export door SR694; `ExportJob` becomes a VecDeque: measure).
28. STRIAPROXYMEDIA — a `proxy` verb renders a quarter-size H.264 of a source once; the face previews from it; export uses the original.

## D. Document, project, face
29. STRIAAUTOSAVE — write `<doc>.autosave.edl` every N edits or 60 s off the face's thread; reopen offers it on the message row.
30. STRIARECENTFILES — `~/.config/stria/recent` (5 entries), `open-recent <n>`; stria-view lists them.
31. STRIAPROJECTBIN — a source bin pane listing the Sources (reel, duration, type) with drag-to-timeline (SR339's split surfaces as the precedent).
32. STRIAKEYBOARDMAP — `~/.config/stria/keys` remapping the KEYS table (SR744; the owed NLE5D-KEYS-CONFIG in the row: build it).
33. STRIATIMECODEDISPLAY — a timecode readout at the playhead (SR747's message row, left half); `tc` toggles frames / timecode / seconds.
34. STRIAJKLTRANSPORT — J / K / L shuttle (reverse / stop / forward at 1x, 2x, 4x on repeats) through the Player's `set_rate` (media.rs :552 EXISTS).
35. STRIAHELPSEARCH — `help <word>` filters the help rows (SR744 HELP table) by substring.

## E. Media (stria-side; the cloud seat owns h264enc_core / hevc_core internals)
36. STRIAEXPORTAACBITRATE — `--audio aac:KBPS` on the face's export verb (the CLI has it: measure the face's export_usage word list).
37. STRIAIMPORTIMAGESEQUENCE — `open` of a `frame_%04d.png` pattern as a video source (a PNG-sequence demux arm in Sources; `lux::decode_png` EXISTS).
38. STRIAIMPORTWAV — a `.wav` as an audio source (measure whether demux_core has a WAV demux; if not, ONE 44-byte header reader + PCM frames).
39. STRIASOURCEINFO — `info` shows the selected source's `summary_parts` (media.rs :1292 EXISTS) on the message row for 5 s; stria-view `"info"`.

## F. Cross-cutting
40. STRIAPERFPAINTBUDGET — a per-paint time readout (ms) on stria-view (`"paint_ms"`) and a pin: a 60-clip timeline paints under 8 ms at 1920x1200 (measure first; name the hot spot if it fails).
41. STRIAFACEGOLDENS — a golden-image table for the face at three sizes (320x160, 1280x720, 1920x1200) with a one-command re-pin script under unaos/scripts (the kernel face shares them: BP-handlers-4).
42. STRIAUNDOHISTORY — `history` lists the undo stack's last 10 ops with their words on the message rows (needs op names on the undo entries: measure what the stack stores).

When fewer than ten unstarted entries remain, the local seat appends ten more HERE from the owed clauses in handlers.md
L104 / L90 and pushes the file. There is no last arc.
