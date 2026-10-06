# WIREDIET (rmbp-ledger B461) — design

**Finding (flights 24/25, `f24-boots.log` + `f25-boots.log`, 72227 lines across six boots, read with awk index).**
The heaviest tags: `[vugfps]` 7588, `USB-DEBUG:` 6576 (the bench's knob, not this arc), `:: VUGART:` 6346,
`[wpace]` 6213, `[wc-h]` 2884, `:: BPACE:` 1491. Cadence measured on f25 (inter-arrival per tag):
- `[vugfps] wf=` — every second per vug: the "on change" test is exact, and a 30 fps window jitters 29..34
  (`2 33;2 31;2 30;2 33;2 34;...`), so it prints every second while nothing moved. (`user-vug/src/main.rs` fps_refresh)
- `:: VUGART:` — every 64 frames per vug (0–1 s apart at 30–100 fps), whatever the verdict. (`art_score`)
- `[wc-h] rollup` — the census refresh at `CENSUS_PERIOD_US` = 2 s (126 of 199 gaps are 2 s); its delta gate
  counts presents, which move every frame, so it is never quiet. (`video/wcg.rs` census_refresh)
- `[wpace]` — ALREADY a 5 s rollup (`WPACE_ROLLUP_MS` = 5000) and dirty-gated (no presents, no block): compliant, untouched.
- `:: BPACE:` — NOT periodic: one line per boot phase (each phase name occurs once per boot, six per flight pair);
  the review's "per-second" label does not fit it. Untouched.

**Seam.** Kernel-by-ruling: each printer keeps its own format; only WHEN it prints changes. Each throttled
printer remembers what it last said in ONE atomic (value and time packed in a u64, or the verdict fold per row).

**Rule applied.** A periodic line prints when its value MOVED (a verdict-bearing change) or when 5 s have
passed since it last spoke and something differs; never every second on jitter.

**Milestones.**
- M1 `[vugfps]`: print when the rate moves by more than a tenth (min 2/s) of the last printed value, or after 5 s
  if it differs at all. `VUG_SAID: AtomicU64` = (tick of print << 32) | value.
- M2 `:: VUGART:`: the power-of-two lines stay (frames=1,2,4,... the first-frame witness ST88 quotes); the
  every-64-frames line prints only when `mixed_frames` changed since the last line or 5 s have passed.
  `ART_SAID: AtomicU64` = (tick << 32) | mixed.
- M3 `[wc-h] rollup` refresh: the heartbeat is 5 s (`WIREDIET_HEARTBEAT_US`); the 2 s period stays for a
  window whose verdict-bearing counters (torn, declines, stalls, longpres, gpu_fallback) moved since its last
  rollup. `H_SAIDFOLD: SegVec<AtomicU64>` (one per row) holds the fold last printed. The latched first rollups are untouched.
- M4 `[flightring] diet lines=<n> up_ms=<ms> per_min=<n>` once at the desktop (`boot::ignite`), from
  `serial_ring::SUBMITTED` (every `_print`), so the next review reads the diet in one line.

**Witness (the next flight).** `[flightring] diet lines= up_ms= per_min=` once beside `[boot] phase=desktop`;
`[vugfps]` lines per vug drop from one a second to one per real move; `[wc-h] rollup` `age_ms=` steps of ~5000
on a steady window (2000 when `torn=`/`declines=` moved).

**STATUS rows touched (format unchanged, every quoted line still prints).** ST88 (`:: VUGART: frames=1 ...` —
frames=1 is a power of two, always printed); ST176 (`[wc-h] vbl_src=` — a different printer, untouched);
ST76 (`:: BPACE:` — untouched). No other ST row quotes `[vugfps]`, `[wc-h] rollup`, `[wpace]` or `[flightring]`.

**Owed.** `USB-DEBUG: KEY` is the bench's (drop `UNAOS_USBDEBUG` from the metal line). WIFI5's capture boot is
not comparable with the others. The rest of the top-25 tags (`:: kepler:`, `[hda]`, `[serialdoor]`, `:: gen7:`,
`:: SMC-SCOUT:`, `:: PWR:`, `[lag]`) are not named by F5 and were not touched.
