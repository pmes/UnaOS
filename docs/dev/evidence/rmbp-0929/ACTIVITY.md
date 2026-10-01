# ACTIVITY (R75) — the wire, on the glass

## Design
- Finding: flight 15, Peter: "smp is still weird though. seems like it should spread the load better" — the serial `:: SMPLOAD:` and his eye disagreed twice. Give him the same census, drawn.
- Mechanism: `video/activity.rs` (FILEVIEW pattern: surface allocated once, painted in place, `font::draw_text` + filled rects, no per-second allocation). Census: `arch/x86_64/sched.rs` `core_load`/`run_queue_len`/`migrations_total` (new tail accessor over `STEAL_MOVES`), `syscall.rs` tail `act_proc_rows` over `PROCS` + `act_kill` (ACL: root, or `bg_owned`; kill = `wc_close_click` arm), `allocator::heap_census(4096)`, `wcpar::workers`, per-window presents from `activity::note_present` (hooked beside `wpace_note_present` in both present syscalls), `users::whoami`, `bootpace::last_stamp`, `arch::ms`.
- Hooks: `quarry::live` key_route / press_route / service (the FILEVIEW/TEXTEDIT chain); verb `activity` (shell.rs + midden_core HOST_VERBS); `tests` fixture `activity`.
- Milestones: M1 module + verb + census + keys + witness + fixture + spec pin (one commit). Dock tile: NOT done — `PinnedApp` is a closed two-variant enum with launch/quit accounting per tile; a third tile is its own arc.
- Gaps stated: `[comp2] pass_us` and `[wpace]` counters are drained by their serial rollups, so the window shows its own presents/s (hottest window) instead; per-process cpu-ms is not tracked (CPU column = the core a process is on right now).
- aarch64: bars from `arch::sched::core_load`; migrations n/a; no process table (says so).
- Witness: `:: ACTIVITY: cpus=<n> procs=<n> heap_used=<KiB> repaints=<n> -> PASS ::`. Pins: `x86-wc.spec` REQUIRE `:: ACTIVITY: cpus=`, FORBID `-> FAIL`.

## Written
Boot 17 (`tests activity`, or the `activity` verb) should show `:: ACTIVITY: cpus=8 procs=<n> heap_used=<KiB> repaints=3 -> PASS ::` (the open line shows repaints=1).
