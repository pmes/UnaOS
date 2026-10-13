# DESIGNFIX — RESUME (stopped by the seat 2026-10-14: wrong model; resumes on Opus)

**RESOLVED** by the Opus resume: the review changed one call (`winmenu.rs:1686` open-title ink → `theme::ink_on_accent()`,
DESIGNFIX.md §2), the ledger row B649 landed, both arc compiles rc=0, `./arroyo gates` 16/17 (the `deps` red is
upstream tokio 1.53.3, no file of this arc), committed on `exec-rmbp-designfix` from `8da376cb`. Kept as history.

Branch `exec-rmbp-designfix`, worktree `/home/user/exec/designfix`, parent `8da376cb`. Arc:
`/root/unaos-bench/scratch/cloud/wave34/designfix.md`. NO compile was run (none started); no gates, no linters run.

## DONE (all edits in the working tree, committed as `wip:`)

1. `unaos/crates/kernel/src/arch/x86_64/syscall.rs` (`winresize_selftest`, ~:29693-29745; LINE-NEUTRAL, line count
   31848 before and after, asserted by the edit script): `e = ui::px(wm::RS_EDGE)`, `bl/bt/br/bb` = mid-band
   coordinates (`ox - 1 - e/2`, `oy - 1 - e/2`, `ox + ow + e/2`, `oy + oh + e/2`) folded onto the `(ow, oh)` line;
   the eight probes read them; `(brx, bry) = (br, bb)`; legs 3/4/5 re-derive the BR grab as `box edge + e/2` (was
   `- 2`). Decision: press points of legs 3-5 moved too (DESIGNAPPLY §9 "the drag legs press at the same points";
   left inside the content `drag` returns None and the gate stays red). Router takes unclamped coordinates
   (`wc_click_route_at` → `hit_test` → `resize_band_hit`, wm.rs:3045), so an off-panel band after the clamp leg
   still hits.
2. `video/winmenu.rs:1686`: `bevel_light()` → `surface_raised()` (value-identical; matches menubar.rs:1974's open
   caption; Dark-set contrast on the accent pill noted as MENUS2's), `title_text_active()` → `text_primary()`.
3. `video/crystal.rs:1550` → `text_primary()`; `:1560` (`sheet::text_disabled`) → `text_disabled()` (the role the
   wrapper's doc names; value moves; `tests menuscraft` paints no labels); `:1748` `chrome_face()` → `surface_window()`.
4. `arch/aarch64/display_tegra.rs:603-608`, `:3916-3921`: `frame_line`→`line_subtle`, `bevel_light`→`surface_raised`,
   `bevel_shadow`→`line_strong` in place (value-identical; probe positions untouched — role calls only per the arc).
5. `unaos/crates/user-aether/src/theme.rs:80-101`: the eight Tok aliases → their targets (palette_core:128-135),
   user-una's spelling. Not compiled (the arc's compile lines are kernel-only).
6. `video/wcf.rs`: `chrome_probes` rewritten against `desktop::chrome_row` (edge_left, edge_right, strip_top,
   strip_bot, title_rule — all exact tokens, see DESIGNFIX.md §3), `_foc` unused, `use super::wm::TITLE_H` only,
   doc table rewritten, skip guard `bw < TITLE_H() + 4`, a `pt=band -> BAND` reading added after `pt=content`,
   the `grad_max` comment updated. `ceramic`, `BEVEL`, `title_row_color` no longer read by the probes.
7. `docs/dev/evidence/designfix/DESIGNFIX.md` written: the legacy→semantic table with file:line, the use counts
   (0 in this arc's files; 16 notify.rs, 3 menubar.rs, 1 quarry/toolbar.rs, 6 Tok reads in softy/palette.rs remain —
   all outside the arc's FILES, so the aliases cannot go yet), the wire shapes.

## NOT DONE — the exact next steps, in order

1. Ledger row B649 in `docs/dev/OS/rmbp-ledger.md`: append after line 680 (`| B645 …`, the last B row before `## C.`).
   7 cells: `| B649 | **DESIGNFIX — …** | shared (video/, arch/x86_64/syscall.rs, arch/aarch64/display_tegra.rs) |
   rmbp | fixed-unflown — DESIGNFIX on exec-rmbp-designfix @the commit carrying this row (parent 8da376cb):
   compiled (<both commands> rc=0), proof owed to the fold + one sentence | x86 QEMU, then the rmbp bench and the
   Orin (…wire…) | — |`. No `|` in a cell, no claim word.
2. Compile ONCE, both arc lines (x86 then aarch64, shared `CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave34/target`,
   never rm). `unaos/target/user_blob.bin` exists in the worktree (51 bytes), no copy needed. Expected risks:
   none known; `_foc` rename and the removed imports in wcf.rs are the only API-shaped edits.
3. Pre-commit leg per the UPDATED wave head rule 12: `cd unaos && ./arroyo gates > gates.log 2>&1; echo rc=$?`
   and quote `GATES: 17 run 17 green -> PASS` (the coordinator withdrew "do not run ./arroyo gates"; QEMU, cargo
   test, ./arroyo check and suites stay forbidden). Optionally the three linters' lines too.
4. Squash/replace the `wip:` commit with the real one: `video: DESIGNFIX — move the winresize probes into the outside
   band, name the semantic roles, re-point CHROME-TRUTH at the frameless head`; body = the design (DESIGNFIX.md §1-3
   is the written design); the two attribution lines (Opus writes Opus). Do not push.

## Learned, not in the arc file

* `wcf.rs` registers no `tests` name; `chrome_truth` runs from `wm.rs:6510` on every composite and its reader is
  `scripts/specs/pi4-regression.spec:903-905`; x86's reader for winresize is `scripts/specs/x86-wc.spec:44`.
* `ON_FILL == PURE_WHITE` (palette_core:69/88); the menuscraft white census scans only sheet rows.
* `rs_zone_of` (wm.rs:29675) classifies over `rs_band_box` (wm.rs:30536); with `e = ui::px(6)`, `e/2` past the edge
  is inside the band at every scale; `-1` on the left/top keeps it outside the box when `e` rounds to 1.
* `draw::rrect_paint` writes exact RGB at alpha 255 / full coverage; `stroke_rrect` width 1 touches x=0, x=bw-1 and
  row 0 only on interior rows — that is why the new probes avoid x=0 and row 0.
* Last compile command: none run. rc: n/a.
