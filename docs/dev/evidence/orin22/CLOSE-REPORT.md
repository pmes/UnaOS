# orin 22 — close report (2026-09-08, 14:10Z → ~18:00Z; closed at context limit)

Record of the round: `~/unaos-bench/scratch/orin22/BULLETIN.md` §0–§22. Next: `~/.claude/plans/unaos/batons/orin-23.md`
(support seat for rmbp 17). Nothing landed to `hw-jetson` (98213b7f) or `main` (c7407753) this round except this report.

## Direction (Peter, verbatim in the bulletin)
"Boot cold, boot dumb, presume nothing about the machine." Root is the disk this kernel was FOUND on, by content — no
board, slot, serial, knob, or boot method in the decision. Another UnaOS disk is home soil (any number; first found is
root; the rest mounted under `/volumes/<LABEL>`); any other disk is a stranger never touched on the kernel's initiative.
Self-hosting with healing grows UnaOS; nothing of it is a bench appendage.

## What landed on branches (verified by `git log`; none pushed by the seat)
| branch | tip | content | review |
|---|---|---|---|
| `exec-orin22-bootroot` | 600887c2 | BOOTROOT (b0536d83/b1885dbc/b12decbb) + KEEP13 merge 26f6ee64 + HOMESOIL 606b9040 + LABELMOUNT 9832fe12 + VERSIONWIN 907435cd + QEMU-FAST 6be107cb + 600887c2 | b12decbb ACCEPTED by rmbp 16; 606b9040..600887c2 UNREVIEWED |
| `exec-orin22-keep13` | 51f06bea | 13 direction-clean commits of the orin 19–21 branches (TEARSCOPE, MENUOWN, PANICLOC, fitsland…) | full battery green; rmbp 16 PASS; ON ORIGIN |
| `exec-orin22-matrixpar` | 5c1f13d0 | cfg matrix P-wide in per-slot dirs, opt-in (default = old serial path) | rmbp 16 ACCEPTED |
| `exec-orin22-lawsnom2` | 66523c05 | docs/dev/LAWS.md +134 (five bullets) | docs-only |

## Gates
- BOOTROOT (b12decbb): check 0 ❌ (55 legs) · kernel8-test 300 **125/125, Pi floor unmoved** · test-arm 0 · esp-jetson
  plain (banner `…,sdmmc`) · three mutations RED (byte flip, decoy, memory side — the last dropped MBENCH to 121/125).
  Witness, Pi in QEMU: `[vfs] root = boot volume serial=0xf3d9b41a source=global match=/KERNEL8.IMG unafs=present
  matches=1 window_off=0x80000 window_len=4096 …`.
- INTEGRATE (600887c2), focus gate only (Peter: focus platform only until metal results are in): check 0 ❌ (59 legs),
  GATE-KNOB OK, GATE-LEDGER OK 126 · esp-jetson plain · strings 12/12.
- KEEP13: check 60 legs · `UNAOS_WC=1 test` (banner `wc`, DOCK/WINMENU PASS, crystal `menu_x=0 glyph_x=12
  anchor=panel-left`) · test-arm 0 · kernel8-test 125/125 · esp-jetson.
- MATRIXPAR: RED-first (named red leg; vanished leg fails the count); orin 21's "155 s warm" WITHDRAWN — warm serial
  is 3.1 s; the real win is cold 387 → 152 s.

## Staged, not flown
`~/unaos-bench/scratch/orin22/stage11/render11-candidate/` (kernel.elf sha256 6dbeda7d…c58c9d), scorer11.sh
(21-row mutation matrix), FLIGHT-render11.md (incl. FRIEND-DIFF three-boot leg). Card write = Peter, host sudo.

## Known gap in the staged candidate
Clone-vs-alias dedupe (integrate/BRIEF-AMENDMENT-01.md item 5) is NOT in: two cloned cards share (num_blocks,
BS_VolID) and would be deduped, hiding one from `/volumes`. Root unaffected. First job of the next focus turn.

## Post-metal (per Peter's ruling, run only after the metal result)
kernel8-test both modes + fast-mode m1/m2/m3 · finder mutations on the Pi leg · friend-disk mutations · test-arm ·
`UNAOS_WC=1 test` + crystal · x86 sizes. Their rc lines are named in the landing report or there is no landing.

## Peers
pi 9: no Pi row moved, no Pi file touched, grant #10 unspent (conditions in the baton), nothing owed either way.
rmbp 16 (archived at context limit): BOOTROOT, KEEP13, MATRIXPAR accepted; their gate-divergence census (seven gates
exist only on hw-rmbp) and the `UNAOS_NOSDMMC` k8-reach registry row are theirs at their landing.

## Mistakes this round (for the next seat)
Four BOOTROOT briefs, two fast-mode briefs, a matrix default rework — each spawned before the design closed, each
spending a battery on a dead design ("the design closes first", bulletin §19). Bare cross-seat ids. "Operator-driven
= safe" (backwards). "60 legs" (56/55). A kernel8-only fast mode. The 81×. Bulletin timestamps written as sequence
labels, not clock readings (corrected at the bottom of the bulletin).

## Open with Peter
Nothing blocking. Direction items §20–§22 (healing fallback disk; host installer as a handler in a vessel with a
per-host privilege path) are roadmap.
