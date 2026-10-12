# FLIGHT 28 — image 21 on the UNAFS CARD IMAGE (rmbp12flight28, hw-rmbp@c2ff8706), 2026-10-07, main bench

Capture: `f28-boot1.log` (pinned). Medium: the 4.1 GiB card image. Knob line = image 20's, unchanged. Tree = merge20 (thirteen arcs, the flight-27 fix wave) + three seat compile fixes (the fold had never been compiled as a kernel: a duplicated `Vec` import in fs/jobs.rs, two bare `return;` in `winmemory::save_frame`). The seat typed 45 lines into the headless door; Peter did dd / power / the first-user form / the glass.

## Peter, verbatim (the glass)
- "desktop" / "what's going on. there's been a long stall"

## 1. Green — the flight-27 wave landed
- **FINEMOTION PASS**: `:: FINEMOTION: one_count_moved=200/200 ten_count_moved=20/20 floor=none stall_s=0 -> PASS` — every one-count trackpad delta moves the pointer (flight 27: fine motion died); `[tp] mt route=legacy … dx=1 dy=0` on the wire.
- **JOBSCAN off the render thread**: `[jobs] volume=/jobs records=1707 … on=worker chunks=12 max_chunk_ms=119 waited_ms=80 ms=1178` (flight 27: 17 s on the render handler); `[desktop] built … bar_ms=71` by `JOBSCAN … bar_ms=71` (the test itself reads FAIL on its own bound — the state is `landed`).
- **DOCKGONE PASS** (`vacates=0 paints=0 present=1`) · **MENUBAR PASS** (`items=6 overlaps=0 … net:2290 battery:2412 notify:2614 clock:2735`) · **SOUNDOPENERS PASS** (`formats=wav,flac,opus,ogg,mp3,aac,m4a opened=7/7 via=audiocore×6,demux`) and `play TEST.FLAC` → `PLAYCODEC … codec=Flac … -> PASS` (the ear's read owed) · **WINMEMORY PASS** (`restored=ok close_save=ok`) · **VERDICTS PASS** · **PRTSCR PASS** (`door=free`) · **SPLASH PASS** (`steps=3 slowest=stage-resolve:310`) · **JOBSUI PASS** (`items_shown=10 first=LOGINFONT`) · **SMALLFIX7 PASS** (`preheap=parked`).
- **GEN7**: `GEN7LADDER … wake=acked r7=r7-blit-verified -> PASS`, `GEN7R8 … r8-fb-blit-verified -> PASS` (the acked wake, first time). **WIFI5 READY** `… fw=pinned sha=match uploaded_this_boot=1 ucode_rev=666 rx_frames_5s=unpinned(movers=0)`. **BATTERY** live PDTR. NOTICE, QUARRYLIVE, WINDOWCAP, INSTALLBARE (keys 4 ms), BOOT80, KVBLANK, VOLUMES, BRIGHTSTEP, TESTF, PWWIRE, KERNELFONT, LUMENCRASH (1.4 s first line — was 10.8 s), LUMENUX, PLAYWAV, DOOR, LOADER, SANITY, QUIETBOOT 139 PASS. Boot to the form 6.3 s.

## 2. Findings
- **QUEUESLEEP (Peter: "there's been a long stall")**: with the machine IDLE (only SMC/status polls on the wire, `[hid] stall census holders=0 -> CLEAR`), the INPUT QUEUE stage stalled for tens of seconds: `[lag] stall … span_ms=24274 stage=pump`, `span_ms=23614 stage=hid`, `stage=queue stage_ms=7886 queue=7886.5`, and `span_ms=34427 stage=queue stage_ms=35413 queue=35413.7`; a seat-typed line waited 45 s to be taken (`tests inputstall` at 12:35), and five lines of the batch were never taken in their 60 s windows (gen7 at first, inputstall, storm, diag, lumen). No lock holder, no render work: the input pump is not being WOKEN when nothing else runs — input lands in bursts when something else wakes the machine. This is the "mouse barely moves" / "long stall" that survives every fix so far; R101's item.
- **FILETYPES at login**: `:: FILETYPES: types=30 build_ms=16134 blocks=8832 masked_max_ms=666 -> FAIL :: login=full` — a 16 s type-registry build at login (the next JOBSCAN-class cost).
- **HIDSTALL** `hid_stall_s=1 dock_timeouts=1 masked_max_ms=666 -> FAIL` (down from 5 s / 10.9 s). **INPUTSTALL** `key_queue_max_ms=3144` once (the queue sleep). **LUMENFAST** `sight_ms=3139 … -> FAIL` (bound 1000; Holocron's key_ms as predicted). **JOBSCAN** reads FAIL on its own bound though landed (bar_ms=71).
- Render-handler stalls of 1.3–1.9 s right after login remain (`stage=render-handler stage_ms=1903`).
- The fold did not compile until the seat fixed three lines (E0252, E0069 ×2): a kernel build belongs in every fold's legs.

## 3. Owed (the cloud's, in Peter's order)
QUEUESLEEP (the input pump must wake on input, not on other work — 35 s queue stalls on an idle desktop; R101 "once and for all" is THIS), FILETYPES' 16 s login build (off the login path, like JOBSCAN), the render-handler's 1–2 s stalls at login, LUMENFAST, HIDSTALL's dock timeout, JOBSCAN's own bound, a kernel build in the fold legs. Then Peter's glass reads (pointer by the finger, bar/dock/menubar, the charger's watts, the lid, Cmd-Tab, the Player, the dongle after login).
