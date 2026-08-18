# BRIEF — kepler-display pull 4: EVO core-channel read-out (read-only)

Lane: **kepler-display** — `unaos/crates/kernel/src/gpu/kepler_display.rs`
ONLY. New session? Read `docs/dev/GEMINI/README.md` first, then
`video/INDEX.md`, then `docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #13.

## Why this pull serves BOTH lanes

s13 closed two doors at once: the head-block windows provably do NOT expose
the scanout surface address (display), and CTRL_ADDR TARGET was refuted 12/12
(fence). Both walls now point at the **EVO core channel**, and the s13 recon
proved it is live: `pdisplay_0=917D0210 +40=0000000A evo_0x490=0D0500A9
evo_0x494=00000001`. This pull maps it read-only. Its capture feeds the
display surface hunt AND the fence's disp-era-USERD fallback.

## This pull — two probes, zero writes

**Milestone 1 — dense core-channel window.** Sequential dump of
0x610480–0x6104FC (zeros printed as zeros), two passes with a bounded delay
between (same idiom as pull 3) to separate telemetry from config.

**Milestone 2 — known-value scan.** Sweep 0x610000–0x613FFC (4 KB of words,
bounded, no polling) and print ONLY hits, where a hit is:
- exact match against the key list: 0x00000200, 0x00020000, 0x90020000,
  0x00002D00 (pitch 2880×4), 0x013C6800 (fb size), 0x07380BAF and 0x0BAF0738
  (the s13-proven raster totals in either order);
- OR `(val & 0xFFF00000) == 0x90000000` (BAR-window-shaped address);
- OR `(val & 0xFFFF) == 0x0B40 || (val >> 16) == 0x0B40` (2880-shaped) —
  same for 0x0708 (1800).
This is value-anchored empirics, not borrowed semantics: we search for
numbers we have already proven on this silicon, and report where they live.

## Exact serial markers (verbatim)

- `:: kdisp: evo-core pass<P> off=XXX val=XXXXXXXX ::` (dense rows, off
  relative to 0x610480)
- `:: kdisp: evo-core pass<P> done rows=N ::`
- `:: kdisp: evo-scan hit off=XXXXX val=XXXXXXXX key=<keyname|barshape|w2880|h1800> ::`
- `:: kdisp: evo-scan done range=610000-613FFC hits=N ::`
- keep all existing begin-trace/caps/stat markers unchanged

Cap scan hit prints at 64 with a `capped=true` note in the done line
(absence-honesty: a flood of false hits must be visible as a flood, not
silently truncated without saying so).

## Gates (DONE = all of these)

Read-only: no register writes anywhere. Full-knob check (`UNAOS_IVB
UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check`, both
arches) + builder-path esp-x86 build + `strings` proof of the new markers in
kernel.elf + default QEMU regression green. Commit ALL docs+code; delete
scratch files; `git status` clean. **You do not push — end your report with
"PUSH OWED: <n> commit(s) on UnaOS-gemini".**

Proposal first (`PROPOSAL-kepler-display-pull4.md`, STATUS: PROPOSED) — no
implementation until approved.
