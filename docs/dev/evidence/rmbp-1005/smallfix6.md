# SMALLFIX6 (rmbp-ledger B495) — flight 26's small reds, each cause named on the wire

Cut from 21521a53 (exec-rmbp-merge19). Flight 26 flew image 19 = merge17 (hw-rmbp@b976af1b); this tip is merge19.

## Findings (wire first)
1. **FWVOL** — flown tree had no UnaFS pass (its lines say only `on source=sdhc label='UNAOS'`). On this tip
   `wifi::firmware::stage_attempt` (firmware.rs:848) runs pass 2 `stage_boot_root` (firmware.rs:598, called at :871):
   `/` resolved through the mount table, volume `native`, bound handle == program source (boot 3: `[unafsbind]
   mount=native handle=sdhc`, `[vfs] root mount / = native` at 14:49:33, before the wifi pass at 14:49:53), `SEARCH_DIRS`
   (firmware.rs:160) = `/`, `/B43/`, `/FIRMWARE/` — arroyo's WIFI-FW puts the set at `/FIRMWARE/` on that volume. The
   ESP copy (dev branch stopgap) is REDUNDANT on this tip. No code change; `tests smallfix6` reads the reach live.
2. **LUMENCRASH 12262 ms** — the spawn and window land at 14:56:10, `:: LUMEN: start` at 14:56:22; the wire has no
   split. Lumen's start does: window, Principia bus (prefs get x3 + declare), Holocron bus, key file, TLS roots load,
   history, the 1.7 MiB font read. Named on the wire now (M2); no plain sleep exists on that path (every bus call is
   a blocking SYS_MRECV on a reply), so nothing is cut blind — the next flight's split says which.
3. **OPENERS 22/24** — TEST.WEBM, TEST.MP4: `opener=none(db)` from the flown tree's OPENER_KEY rows (no video
   registrant there). On this tip the Player's resources declare `video/mp4`, `video/webm`, `video/x-matroska`
   (merge18, VIDEOPLAYER B434) and `openers::effective` maps `player` under `videoplayer` (in the x86 shape). Not a
   stale fixture list: a missing registrant on the flown tree, fixed on this tip. The stale `reason=` words
   (`no-opener-in-this-tree(video: Stria's player, SR26)`) become `reason=no-registrant`.
4. **windowcap** — `tests windowcap` -> `TESTS: ran=0`: WINDOWCAP is a boot verdict (`wincap::witness`, latched
   once), no fixture by that name. Now registered (re-reads the live limit, PASS/FAIL), and ANY unknown name says so.
5. **render-handler 6094/6086/6115 ms** — boot 1: root-mount 590 + assoc-seed 5498 = 6088; boot 3: 552 + 5562 =
   6114. The render task made no route/pass/park for the span of `users::service`'s `boot80_root_and_seed` on the
   usb-pump (the flown tree wrote the type registry at boot, 3664 blocks read, 211 written, created=27); the render
   task waits on the UnaFS volume (`[lock] spin-wait name=unafs.rs:567 waiter=9@cpu7` is the render task on the
   volume lock, boot 3 14:56:10). On this tip the boot seed is GONE (FILETYPES B423: built at `login ok` on the
   usb-pump, ASSOCSTAMP skips a matching stamp). The boot line now names the overlap (M3).
6. `tests smallfix6` lists what it checked (M4).

## Seam
Kernel-by-ruling (fixtures and witness lines; no new store). Lumen's split is the program's own line.

## Milestones
- M1 OPENERS reason words; `tests windowcap`; unknown fixture name says NOT-FOUND.
- M2 Lumen split: `[lumen] first_line_ms=<n> start_at_ms=<n> win_ms= bus_ms= key_ms= tls_ms= hist_ms= font_ms=`;
  kernel `[lumencrash] … spawn_ms=<n>` (the ELF load + APPRES's sight inside the spawn).
- M3 `bootstep` keeps the boot's step spans; the lag boot line adds `handler_span_ms=<a>..<b> overlaps=<steps|none>`;
  the login registry build is a logged span (`filetypes-build`).
- M4 `tests smallfix6`.

## Witness (next flight)
`[lag] stall boot_suppressed=<n> worst_stage=<s> worst_ms=<n> handler_span_ms=<a>..<b> overlaps=<steps|none>`
`[lumen] first_line_ms=… win_ms=… bus_ms=… key_ms=… tls_ms=… hist_ms=… font_ms=…` · `[lumencrash] … spawn_ms=<n>`
`:: OPENERS: test_f=24 … opened=24 …` · `:: WINDOWCAP: … -> PASS :: via=tests` ·
`:: wifi: ucode STAGED /FIRMWARE/… on source=boot-root volume=native handle=Sdhc …` ·
`:: SMALLFIX6: fwvol=<…> openers=<…> windowcap=registered notfound=said lag=<…> lumen=split -> PASS|FAIL ::`

## Owed
The 5.5 s registry build itself (one volume transaction at 8 ms/cmd) still runs once per fresh card at `login ok`;
whether the render task waits behind it there is the next flight's `[lag] stall … render=handler` line. Lumen's
cost is named, not cut.
