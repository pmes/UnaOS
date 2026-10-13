# KNOBOFF26 — six knob-off measurements, rmbp local bench, 2026-10-13

Arc KNOBOFF26 (rmbp-queue LOCAL WAVE 2 item 5). Executor branch `exec-rmbp-local-knoboff26`, cut from `2c538013f`.

## Result

Six runs, one per feature. **All six returned `2` (NO VERDICT).** None is IDENTICAL or DIFFERS: knoboff needs
both knob-off builds to compile on both arches, and under the x86 image line the aarch64-virt half never compiles.

| feature | row | arc (current) | baseline | rc | wall | tool verdict line |
|---|---|---|---|---|---|---|
| wc | B542 TWOCURSORS | 9ef4f764b | bf81f7ef9 | 2 | 436 s | `knoboff: NO VERDICT — a knob-OFF build failed (x86 base:1 arm base:1 x86 cur:1 arm cur:1).` |
| desktop_firmware | B548 CURSORARM | ed9d33840 | abcb8d337 | 2 | 398 s | `knoboff: NO VERDICT — a knob-OFF build failed (x86 base:0 arm base:1 x86 cur:0 arm cur:1).` |
| unafs | B549 BOOTLOGCARD | 6372ae0d7 | abcb8d337 | 2 | 262 s | `knoboff: NO VERDICT — a knob-OFF build failed (x86 base:0 arm base:1 x86 cur:0 arm cur:1).` |
| sonde | SR105 SONDE2 | 0069ca008 | abcb8d337 | 2 | 245 s | `knoboff: NO VERDICT — a knob-OFF build failed (x86 base:0 arm base:1 x86 cur:0 arm cur:1).` |
| prtscrst | B556 SCREENSHOTFAT | d20412c54 | cc2b7a8a6 | 2 | 235 s | `knoboff: NO VERDICT — a knob-OFF build failed (x86 base:0 arm base:1 x86 cur:0 arm cur:1).` |
| lumen | B546 DOCKPINS | 8fae9ef37 | bf81f7ef9 | 2 | 218 s | `knoboff: NO VERDICT — a knob-OFF build failed (x86 base:0 arm base:1 x86 cur:0 arm cur:1).` |

## Why each arc was measured at its own commit

The brief's anchor `2c538013f` is a MERGE commit, so a bare `./arroyo knoboff <feature>` refuses with exit 2 because
there is no default baseline on a merge HEAD (see the knoboff header). Comparing `2c538013f` against image 24 would score every
commit since then, unconditional code included, rather than the arc's own claim. So each run moved this worktree's
HEAD to the arc's executor commit (`git switch --detach <arc>`) and named that commit's parent as the baseline.
The worktree was clean throughout, and it went back to `exec-rmbp-local-knoboff26` before this file was written.

**Which rows prtscrst and lumen point at** is the executor's choice, because the queue item names no row for them:
- prtscrst → B556 SCREENSHOTFAT. It is the only arc since image 24 that edits PRTSCR-ST code, and it claims "every edit above the file tail is line-neutral".
- lumen → B546 DOCKPINS. It folds onto the `lumen` row's line and `DP_ALL`'s `cfg!(feature = "lumen")` line, and claims "dock.rs keeps 4683 lines". CURSORSHAPE (B550) also edits a `lumen`-gated dispatcher line, but it is unconditional code.

## Command (identical for all six, from `unaos/` with HEAD at the arc)

`scripts/image-line.env` is byte-identical at all six arc commits and at `2c538013f` (`git diff --stat` empty). The
worktree-isolation hook in this harness refuses `. ./scripts/image-line.env`, so the line's 43 kernel knobs were passed
literally, in the file's order, plus the media path:

```
env UNAOS_WIFI_FW_PATH=/home/pmes/src/github.com/pmes/UnaOS-bunker/drivers/broadcom-b43 \
  UNAOS_WITNESS=1 UNAOS_SELFHOST=1 UNAOS_WCG_PAYGO=1 UNAOS_WCDVALVE=1 UNAOS_LOGTS=1 UNAOS_SMC=1 UNAOS_BT=1 UNAOS_BTC=1 \
  UNAOS_LOGIN=1 UNAOS_WIFI=1 UNAOS_WIFI2=1 UNAOS_WIFI3=1 UNAOS_WIFI4=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 \
  UNAOS_NOASPM=1 UNAOS_KEPLER_CE=1 UNAOS_WC=1 UNAOS_KEPLER_VBLANK=1 UNAOS_DEADMAN=1 UNAOS_LINUXABI=1 UNAOS_BUSREG=1 \
  UNAOS_LUMEN=1 UNAOS_NETRING3=1 UNAOS_SELFDIAG=1 UNAOS_LIDSLEEP=1 UNAOS_VIDEO=1 UNAOS_IVB=1 UNAOS_IVB3D=1 \
  UNAOS_IVB3D_R8=1 UNAOS_IVB3D_BLIT=1 UNAOS_GMUX_IGD=1 UNAOS_BEAM=1 UNAOS_QUARRY=1 UNAOS_FACET=1 UNAOS_FTDIRX=1 \
  UNAOS_USBNET=1 UNAOS_AHCI=1 UNAOS_UNAFS=1 UNAOS_HDA=1 UNAOS_HDATONE=1 UNAOS_UVC=1 UNAOS_IOAPIC=1 \
  UNAOS_AETHER=1 UNAOS_MIDDEN=1 UNAOS_UNA=1 UNAOS_SONDE=1 UNAOS_KNOBOFF_WITH_ENV=1 \
  ./arroyo knoboff <feature> <baseline>
```

`UNAOS_KNOBOFF_WITH_ENV=1` is required here. The line does not compose the default feature set, so without it
knoboff refuses at step (2). Each verdict below is therefore about the IMAGE-LINE knob-off image, not the default image.

Logs: `~/unaos-bench/scratch/knoboff26-1013/<feature>.log`. Kept run directories (flats, censuses, build.log):
`~/unaos-bench/scratch/knoboff/caller-agent-ada75b5e7a859ef98-25a1764297be/run-20261013-{064922Z-253341 (wc),
065708Z-291638 (desktop_firmware), 070424Z-328470 (unafs), 070908Z-356471 (sonde), 071336Z-388639 (prtscrst),
071758Z-407196 (lumen)}`.

## The blocker (one cause, six runs)

`_knoboff_arm_csv` turns the x86 line into an aarch64 set:
`witness,selfhost,wcg-paygo,logts,smc,login,nvidia-kepler,nvidia-kepler-takeover,wc,nvidia-kepler-vblank,linuxabi,busreg,lumen,midden,una,sonde,aether,netring3,selfdiag,lidsleep,videoplayer,intel-ivb,unaos_ivb,gmux_igd,beam,quarry,facet,sdwrite,ftdirx,usbnet,unafs,uvc`.
On aarch64-virt that set does not compile, before and after every arc:
- `crates/kernel/src/fs/appres.rs:528`: `crate::video::facet::win()`, where `video::facet` is configured out.
- `crates/kernel/src/keyring.rs:121`, `:133`, `:455`, `:456`: `crate::arch::syscall`, `crate::bus_route` and `crate::prefs_client` are configured out. These sites are `lumen`-gated. With `lumen` removed (the lumen run), the arm build still fails on `appres.rs:528` alone (1 error).

The wc run also fails on x86. The line minus `wc` reaches `fs/appres.rs:528` (`video::facet` needs `wc`), so
`wc` cannot be knob-off measured under this line on either arch.

Until the arm derivation is a buildable aarch64-virt set, or knoboff can score one arch alone, knoboff has no power
over any image-line feature. That fix is shared code (`fs/appres.rs`, `keyring.rs`, or `arroyo`'s `_knoboff_arm_csv`) and
belongs in a queue, not in this arc.

## Supplementary, NOT a knoboff verdict: the x86 flats the runs left behind

These are `cmp` results on `flat-{base,cur}-x86.bin`. They carry no control, and the warm-up retry never ran,
because `_knoboff_scored_pair` retries only when both arches built. Only a run whose two x86 censuses read exactly
`unaos-kernel` is a warm-against-warm comparison.

| feature | x86 census base / cur | cmp |
|---|---|---|
| desktop_firmware | foreign crates (cold) / unaos-kernel | identical (14066000 B), but the base was cold |
| unafs | unaos-kernel / unaos-kernel | identical (13081712 B) |
| sonde | unaos-kernel / unaos-kernel | identical (14214608 B) |
| prtscrst | foreign crates (cold) / unaos-kernel | 11021001 bytes differ, size 13444136 → 14235180, base cold |
| lumen | foreign crates (cold) / unaos-kernel | 9881135 bytes differ, size 13116512 → 13116000, base cold |
