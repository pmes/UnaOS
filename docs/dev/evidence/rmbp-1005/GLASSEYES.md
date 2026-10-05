# GLASSEYES — EYES can see the metal (rmbp-ledger B343, SR23)

## Finding
EYES (SR23, `tools/eyes/`) scores what a HOST program draws. The kernel already writes the panel to a PNG
(`prtscr.rs` + SHOTZIP + SHOTREGION) and the bench already pulls files off the card, but every capture is a
`SCREEN<n>.PNG`/`SHOT-HHMMSS.PNG` of whatever happened to be on the glass, with a live clock and a live
cursor in it — nothing names WHAT state it is, and nothing compares it to anything.

## The seam
`Kernel — wm` (no handler owns a screenshot of the compositor's own frame). The capture is the EXISTING
PRTSCR job — same panel door, same streaming encoder, same mount-table write, same SHOTZIP/SHOTMOUNT witness —
driven with a NAMED override (`prtscr::capture_named(leaf, name)`, at prtscr's tail; one `if let` in
`Job::begin` for the leaf + name). Nothing new writes pixels a second way. The mask is a second, tiny PNG
written by the same encoder through the same mount table.

New files (CHARTER lines):
- `unaos/crates/kernel/src/video/shotmask.rs` — `//! CHARTER: Kernel — wm` — the state table, the per-state
  mask table, compose + settle, the `shot <state>` verb body, `tests shot`.
- `tools/eyes/suites/metal/` — cases (one per state) + `golden/README.md` (seeded empty).
- `tools/eyes/run.sh` + `tools/eyes/metal.py` — runner stub (this tree has no `tools/eyes`; when SR23 folds,
  the stub's `metal` arm is what `eyes run metal --from` must do — see "design questions").

## Milestones
- **M1** `shot <state>`: `login | desktop | quarry | settings <general|users|display|about> | lumen`.
  Compose (open the window / lock the screen / close kernel windows), SETTLE (the composed panel sampled on a
  16-px grid, masked; settle = 3 consecutive identical samples 100 ms apart, bounded at 4 s — the wm keeps no
  global dirty-region counter, so the frame itself is the signal), capture through PRTSCR to
  `/home/<u>/Shots/<STEM>.PNG`, write `<STEM>.MSK` (a PNG: white = masked, black = scored). FAT's create path
  is 8.3-only (prtscr's documented rule), so `<state>.png`/`<state>.mask.png` are spelled `DESKTOP.PNG` /
  `DESKTOP.MSK`; stems: LOGIN DESKTOP QUARRY LUMEN SETGEN SETUSR SETDSP SETABT. The runner reads either spelling.
  Mask table: every state masks the menu-bar CLOCK, the CURSOR sprite box (padded) and the status GLYPHS area
  (battery item + brightness transient + anything else left of the clock); `login` adds the password field row,
  `lumen` the transcript body (network-fed), `quarry` the size/date columns' right half.
- **M2** `tools/eyes/suites/metal/{suite.toml,cases/metal.toml,golden/README.md}` — SUBJECT kind `png`
  (`{from}/<STEM>.PNG`), ORACLE kind `golden` (`golden/<state>.png`), mask from `{from}/<STEM>.MSK`;
  `tools/eyes/run.sh metal --from <dir>`; `--accept` blesses the golden set from that dir.
- **M3** drift report: `tools/eyes/out/metal/SCORE.md` — per state the mismatch % OUTSIDE the mask, the masked
  fraction, and `<state>.diff.png` (golden faded grey, red where they differ, blue where masked).
- **M4** `tests shot`: shoot `desktop`, decode it back with the kernel's own PNG path (`facet::decode_file`),
  check the mask PNG decodes and covers the clock's rectangle.

## Witness
`:: SHOT: state=<s> file=/home/<u>/Shots/<STEM>.PNG settle=<ok|timeout> samples=<n> settle_ms=<ms> mask=<k>@x,y,w,h;… -> OK|FAIL ::`
and from `tests shot`:
`:: GLASSEYES: states=5 shot=desktop png=ok mask=clock,cursor,glyphs -> PASS ::`

## Stays owed
- Long names (`desktop.png`) wait for FATLFN write; until then 8.3 stems.
- Golden PNGs: the first flown boot blesses them (`run.sh metal --from <dir> --accept`).
- The runner stub folds into SR23's `eyes` binary (`{from}` placeholder + `mask_png` field).
- `shot lumen` needs `lumen` + x86 (no aarch64 LUMEN.ELF); refused by name elsewhere.
