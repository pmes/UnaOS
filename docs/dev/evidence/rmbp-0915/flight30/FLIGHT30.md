# FLIGHT 30 — image 22b on the UNAFS CARD IMAGE (rmbp12flight30, hw-rmbp@a704f79f), 2026-10-12, main bench

Capture: `f30-boot1.log` (pinned; boot 1 only — boot 2 put nothing on the wire). Medium: the 4.1 GiB card image (`UnaOS-rmbp-esp-rmbp12flight30-20261013T0105Z-a704f79`). Knob line = `unaos/scripts/image-line.env` + `UNAOS_AETHER=1 UNAOS_MIDDEN=1` (image 22's line, unchanged). Tree = a704f79f, the cloud's SESSIONWEDGE/RESOLVEHANG fix commit on top of image 22. Build green (gate30s rc=0, banner-cert 52/0, b43 pins MATCH). Stick out, dongle out. Two boots asked: fresh card, then the reboot with una in the store.

## Peter, verbatim (the glass)
- "remote has some fixes for 22 and says this is your next move but they have not been good at this"
- "is it possible you can fix this yourself real quick or is this a huge problem"
- "resolving session hang again"

## 1. Boot 1 (fresh card) — SESSIONWEDGE is FIXED on metal, then a new wedge under `tests queuesleep`
`:: LOADER: firmware->loader=13748ms loader->kernel=2606ms … kernel:2545ms`; `GPUBLIT: selftest=ok -> blitter=gpu`; `:: BOOT: loader->desktop=6161ms total=19910ms lines=142 slowest=users-load:86`; `FIRSTBOOT: stage=installer users=0 -> coherent`; the first-user form (`key latency ms=1–2 n=18`, `input_blocked_ms=0`, `kdf_ms=2407 adduser_ms=1383 on=worker`); `[login] session open user=una role=admin`; `[boot] phase=desktop from=setter at=28119ms`; **`[desktop] built at=login ok bar_ms=38 battery=painted dock=painted pins=7`**; `[login] items n=0`; `[jobs] volume=/jobs records=1754 … chunks=12 max_chunk_ms=69 ms=567 on=worker`; `WINDOWCAP: fixed_cap=none limit=145 procs=46 from=mem:185`; wifi ucode STAGED + set-validation VALID + WIFI5 capture; the compositor's rollups (`wedge1 … -> QUIET`, `TEAR-FREE`), `[ptr] fine n=159 moved=159 lost=0 -> OK` at 19:16:18 (44 s past ignition), `PWR: state=plugged (charging) 12400–12552 mW` live. The wire ran 96 s past `[login] desktop bare` — the point where image 22's boot 1 went silent. The first fix is proven.

Then the seat typed `tests queuesleep` (19:16:54; `[midden] cmd="tests queuesleep" -> Host verb=tests`, `:: TESTS: run queuesleep ::` at 19:16:55). From that second: `[lag] stall … stage=comp stage_ms=967 → 1967 → 2967` (the compositor stalled while the test ran), `key→echo timeout ms=3227 reached=app`, `LAG: … timeout=1`. The seat's next line `tests loginrender` (19:17:03) reached the door (`[serialdoor] line verb=tests -> shell(headless)`) but `[shelltask] queue verb=tests queued=1` — queuesleep was still running. Last rows 19:17:08–10: `[hid] stall census -> CLEAR`, PWR rollup, `cursor11/wedge9/ptrpaint/wedge11` scope=desk; then NOTHING — no panic, no stack row, no more seven-second rollups. The door never echoed loginrender. Peter's glass: wedged.

## 2. Boot 2 (reboot, una in the store) — RESOLVEHANG is NOT fixed
Zero bytes on the wire (the capture alive and holding ttyUSB1, same adapter ABAFUJCO). Peter: "resolving session hang again". Flight 29's shape exactly: the splash stops at "Resolving the session", before the FTDI binds, so the hang is silent by construction. a704f79f's stage-resolve fix did not reach metal on this path.

## 3. Findings
- **SESSIONWEDGE fixed (B525 closed on metal)**: desktop built at login, jobs volume scanned, services and rollups running, pointer alive 96 s past `desktop bare`.
- **QUEUEWEDGE (new, boot 1)**: `tests queuesleep` holds the compositor (comp stall grows 1 s/s from the run line) and the kernel goes silent 15 s later with no witness row. The test that was written to measure an idle-queue stall is itself the stall. Until it is read, queuesleep is OFF the seat's list (the next flight's list starts at loginrender).
- **RESOLVEHANG still open (B524)**: the second boot of a card with a user still hangs at stage-resolve, silent. The fix must be proven on a card with a users store — the cloud has no such leg; the wire cannot see this stage, so the fix needs a witness that survives (the splash word is the only one).

## 4. Owed (the cloud's, in Peter's order)
RESOLVEHANG (still), QUEUEWEDGE (the queuesleep test), then the whole flight-28 read list (loginrender, hidstall, jobscan, renderwedge, quarry2, filetypes, lumenfast, aether, midden, finemotion, inputstall, door, loader, gen7, wifi, battery, installbare, loginfurn, quietboot, boot80, volumes, soundopeners, FLAC, font, lumen, hda, playwav, storm, diag) and USBNET RX. A kernel build AND a two-boot metal-shaped leg in every fold; a test that can wedge the kernel needs its own stack/lock witness before it ships.
