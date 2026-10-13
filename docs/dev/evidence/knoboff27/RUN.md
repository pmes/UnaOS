# KNOBOFF27 — knob-off verdicts for the arcs since image 27, and the banner-cert tokens, rmbp local bench, 2026-10-13

Arc KNOBOFF27 (rmbp-ledger B644, rmbp-queue LOCAL WAVE 3 item 8). Executor branch `exec-rmbp-local-knoboff27`, cut from
`6f13e5e92`. Seat rmbp-0915/local3. Measurement only: this file and ledger cells, no code.

## Result

- **14 tool verdicts measured** (`./arroyo knoboff`, each with `warm=yes` and the control fired): **10 PASS (rc=0), 4 FAIL (rc=1).**
- **2 arcs not measured**, because their code is unconditional and knoboff cannot measure that (LAWS §5, "Knob-off identity cannot certify
  a change to unconditional code"): TPSTREAM and PTRPRESS2.
- **The image-line form still gives no verdict at `6f13e5e92`** (rc=2, below). So every verdict here is about the
  **DEFAULT image**, with no `UNAOS_*` knob in the environment. That is the classic "default OFF ⇒ byte-identical to baseline"
  claim. It is not a claim about the image-line knob-off image.

## FIRST: the image-line form at 6f13e5e92 — still NO VERDICT

The command, from `unaos/`, at `6f13e5e92` (clean, single parent, so the default baseline is `HEAD~1` = `eb12b7b6b`). The harness
refuses `. ./scripts/image-line.env`, so the file's 49 `export UNAOS_*=1` knobs were passed literally in file order. They were
extracted with `grep -o '^export UNAOS_[A-Z0-9_]*=1' scripts/image-line.env`:

```
env UNAOS_WIFI_FW_PATH=/home/pmes/src/github.com/pmes/UnaOS-bunker/drivers/broadcom-b43 <the 49 line knobs> \
  UNAOS_AETHER=1 UNAOS_MIDDEN=1 UNAOS_UNA=1 UNAOS_SONDE=1 UNAOS_KNOBOFF_WITH_ENV=1 ./arroyo knoboff wc
```

Verdict (wall 186 s): `knoboff: NO VERDICT — a knob-OFF build failed (x86 base:1 arm base:1 x86 cur:1 arm cur:1).` **rc=2**

Same cause as KNOBOFF26 (`docs/dev/evidence/knoboff26-1013/RUN.md`), and none of it has changed:
- x86, line minus `wc`: 1 error, `crates/kernel/src/fs/appres.rs:528:39` `cannot find facet in video`. The site is
  `#[cfg(feature = "facet")]`, but `video::facet` needs `wc`.
- aarch64-virt (the line mapped by `_knoboff_arm_csv`): 6 errors. One is `fs/appres.rs:528`, and five are in `keyring.rs`:
  `:121` (`arch::syscall`), `:133` (`bus_route`), `:455` (`prefs_client`), and two at `:456` (`E0277 [u8]` unsized).
  The armed arm build fails on the same six errors, so arming `wc` does not fix the aarch64 side.

`./arroyo knoboff` has **no single-arch mode**. Read at `unaos/arroyo:13781` onward: `_knoboff_scored_pair` always builds x86 then
arm, and any failed knob-off build returns 2. Under the line, then, knoboff has no power over any feature on aarch64. For `wc` it
has no power on x86 either. The fix is the same shared-code item KNOBOFF26 named (`fs/appres.rs:528`'s cfg, `keyring.rs`'s lumen
sites, or `_knoboff_arm_csv`). It is not this arc's.

**What was measured instead.** The default environment builds on both arches, so it gives the tool's own verdict, control
included. A KNOBOFF26-style x86 `cmp` would carry no control. So each arc got ONE run, in the default environment, at
its FOLD commit with the fold's FIRST parent named as the baseline. Each fold is an arc-into-track `--no-ff` merge, so parent 1 is
the tree without the arc. That follows the knoboff header's "landing" case. The worktree was detached to the fold for each run
and returned to `exec-rmbp-local-knoboff27` afterwards (`run0.sh`: `git switch --detach <fold>`; `./arroyo knoboff <feature> <p1>`;
`timeout 600`). No run reached the 10-minute wall. Default set: x86 `ehcihid,kbdwit,sdw,sdhcblk,smolnet,sdwrite`, arm `sdwrite`.

## The verdicts

Each line is the tool's own. Every run printed `warm=yes compiled_baseline=[unaos-kernel|unaos-kernel] compiled_tree=[unaos-kernel|unaos-kernel]`.
Every control fired (`armed≠off: YES`) on x86, and on arm too, except for x86-only features (gen7r9, wifi9), where arm reads `no`.

| arc | row | feature | fold (HEAD) | baseline (fold^1) | rc | wall | tool verdict |
|---|---|---|---|---|---|---|---|
| NOTIFYCENTER | B622 | wc | 4a004859e | d76f41a24 | 0 | 201 s | `knoboff: PASS — 'wc' OFF is byte-identical to d76f41a2 on both arches; control fired, cache warm.` x86 5851fecc…/4704328, arm 57ca1f89…/4613496 |
| DRAGNOMOVE | B620 | wc | 578877501 | 4a004859e | **1** | 147 s | `knoboff: FAIL — the knob-off image is not the baseline's.` x86 MOVED 2740576 B differ, size 4704328→4703824; arm MOVED 1520059 B differ, size 4613496→4612352 |
| GEN7R9B | B626 | gen7r9 | 6885dbadc | 67f3c8382 | 0 | 139 s | `knoboff: PASS — 'gen7r9' OFF is byte-identical to 67f3c838 on both arches; control fired, cache warm.` x86 160ce202…/4881928, arm 3cdb9723…/4614408 |
| KEPLERGR4 | B627 | nvidia-kepler | 4c2ed56ce | 6885dbadc | 0 | 159 s | `knoboff: PASS — 'nvidia-kepler' OFF is byte-identical to 6885dbad on both arches; control fired, cache warm.` x86 de6828fc…/4881928, arm a34c431d…/4614408 |
| WIFI9 | B625 | wifi9 | 1623d1c35 | 7855e0784 | 0 | 124 s | `knoboff: PASS — 'wifi9' OFF is byte-identical to 7855e078 on both arches; control fired, cache warm.` x86 492331c4…/4881928, arm 55531826…/4614408 |
| DOCKSHELL2 | B621 | wc | 15fdfbde8 | cec0c844e | 0 | 205 s | `knoboff: PASS — 'wc' OFF is byte-identical to cec0c844 on both arches; control fired, cache warm.` x86 919e4fb9…/4881928, arm 2d8fd1e8…/4614408 |
| GLASSKIT | B628 | wc | 33eb71dc9 | eed033e4d | 0 | 239 s | `knoboff: PASS — 'wc' OFF is byte-identical to eed033e4 on both arches; control fired, cache warm.` x86 22613fa6…/4882696, arm dfbdc5b8…/4614536 |
| SOFTY | B629 | wc | 817f7945c | d77b5e670 | **1** | 271 s | `knoboff: FAIL — the knob-off image is not the baseline's.` x86 MOVED 2881408 B differ, size 4882696→4883208; arm MOVED 746430 B differ, size 4614536→4614472 |
| WINRESIZE2 | B635 | wc | fee257de3 | 529157831 | 0 | 194 s | `knoboff: PASS — 'wc' OFF is byte-identical to 52915783 on both arches; control fired, cache warm.` x86 6c25fb13…/4883208, arm 79bec328…/4614472 |
| DOORSPLIT | B634 | wc | 62206a712 | fee257de3 | 0 | 167 s | `knoboff: PASS — 'wc' OFF is byte-identical to fee257de on both arches; control fired, cache warm.` x86 1341d4bd…/4883208, arm 30008cac…/4614472 |
| DHCPREPLUG | B632 | usbnet | e524f7926 | 1d3ec3bc8 | **1** | 193 s | `knoboff: FAIL — the knob-off image is not the baseline's.` x86 MOVED 16745 B differ, size unchanged 4886408; arm MOVED 37246 B differ, size unchanged 4614600 |
| SETTINGSCRAFT | B630 | wc | 3c1e7c2b4 | 8f4494d95 | 0 | 184 s | `knoboff: PASS — 'wc' OFF is byte-identical to 8f4494d9 on both arches; control fired, cache warm.` x86 42ce6469…/4886408, arm bd1419ba…/4614600 |
| DESKTOPCRAFT | B631 | wc | 0a9c33406 | 3c1e7c2b4 | 0 | 157 s | `knoboff: PASS — 'wc' OFF is byte-identical to 3c1e7c2b on both arches; control fired, cache warm.` x86 aa684330…/4886408, arm 9754e42c…/4614600 |
| STRIAMETAL | SR139 | stria | e6eab096d | 440907146 | **1** | 273 s | `knoboff: FAIL — the knob-off image is not the baseline's.` x86 MOVED 2636316 B differ, size 4703560→4704328; arm MOVED 170073 B differ, size unchanged 4613496 |
| TPSTREAM | B624 | — | 55e4b6e80 | 578877501 | — | — | **unmeasured: unconditional code.** `drivers/ehci/mod.rs` sits under `ehcihid`, which is ON by default and on the line, and `pointerfeel.rs` is unconditional (`lib.rs` `pub mod pointerfeel;`). No knob takes the change out of any image, so no knob-off image exists to compare. |
| PTRPRESS2 | B633 | — | 1d3ec3bc8 | 62206a712 | — | — | **unmeasured: unconditional code.** Every hunk is in `impl XhciController` or the free fns of `drivers/xhci/mod.rs` (`drivers/mod.rs` `pub mod xhci;`, no cfg). |

### What the four FAILs are (read from the fold diffs; none is a fault in the tool)

The rc=1 verdicts are true statements about the default image. They are what LAWS §5 says knoboff gives for code every
image compiles:
- **DRAGNOMOVE**: `pal.rs:1224` `push_locked` gains `if let Event::Button(mask) = event { cursor::set_button_level(…) }`, folded
  line-neutrally onto the `is_key` line. That keeps the panic Locations, but the statement is unconditional code, so the image
  grows. `drivers/ehci/mod.rs` is `ehcihid` (default ON). The row never claimed knob-off identity.
- **SOFTY**: edits `libs/sys/prefs_core` (`appearance.rs`, `schema.rs`: `LOOK_KEY`/`LOOKS`/`system.look`) and
  `libs/sys/palette_core`. Both are non-optional kernel dependencies (`Cargo.toml:3893`, `:3897`).
- **DHCPREPLUG**: the only source delta the default image compiles is `crates/net/src/dhcp.rs`. It gains 392 lines at its
  **TAIL** (the file was 363 lines at `1d3ec3bc8`, hunk `@@ -363,0 +364,392 @@`), so no `panic::Location` in that file moves.
  `tests.rs:336` and `drivers/xhci/usbnet.rs` are `usbnet`-gated, which is off by default. `net` is a non-optional dependency
  (`Cargo.toml:3945`). Even so, the image moved with **its size unchanged on both arches** (x86 4886408, 16745 B differ; arm 4614600,
  37246 B differ). **Cause not isolated here.** It is not a kernel-tree line shift. The likely candidate is that a changed
  workspace crate re-hashes its codegen (symbol order or names) even when the new code is dead. If so, a tail append to a
  non-optional workspace crate cannot keep the default image byte-identical. That is worth one probe by whoever owns knoboff:
  the same tail append to `crates/net` with no caller, measured the same way.
- **STRIAMETAL**: unconditional additions in `arch/x86_64/syscall.rs` (x86 size grows by 768 B), `arch/aarch64/syscall.rs`,
  `drivers/hda_play.rs`, `fs/assoc.rs` and `crates/una-abi` (arm: same size, 170073 B differ; which edit causes it was not
  isolated).

The tool's census reads `unaos-kernel` only even for the arcs that edit a workspace crate (SOFTY, DHCPREPLUG, STRIAMETAL). The
first pass of each scored phase compiled the edited crate, and `_knoboff_scored_pair` discarded that pass as a warm-up and scored
a kernel-only second pass. So both scored images came from the same cache state, and the verdict compares the sources.

## SECOND: banner-cert `unmeasured-here` tokens on the bench's image-27b artifact

Artifact (read-only, nothing built there): `~/unaos-bench/scratch/rmbp-0915/gate9/unaos/target/x86_64_esp/kernel.elf`, sha256
`8fea1375ab3d13d70bc2ef757b193d5ffe01643785740126ee8c6a96c4585278`, 15746264 B, mtime 2026-10-13 08:52 MDT. The gate tree's HEAD
is `0d88e0c9e` (`registry: BOOTVERDICTS2, KVMLAT2, PISPEC cut from 37adc602`), an ancestor of `6f13e5e92`. **It predates every
fold above**: `e6eab096d` STRIAMETAL, `6b35b738d` INSTALL4 and `f08fe0308` are NOT ancestors of it. Each token was counted with
`LC_ALL=C grep -a -o -F '<witness>' kernel.elf | wc -l`:

| feature | witness (banner-cert.sh bc_table) | hits | reading | proposed bc_table change (the seat owns banner-cert.sh; not edited here) |
|---|---|---|---|---|
| wifi9 | `:: wifi9: rx NOT ATTEMPTED reason=no-tune` | 0 | the artifact's tree has no WIFI9 (folded later, `1623d1c35`). Also `wifi9` alone: 0 | none: stays `unmeasured-here` until an image-27f build; re-grep there |
| gen7r9 | `[gen7] r9 ring=rcs r6=` | 0 | `0d88e0c9e` carries `gen7_r9.rs` with 3 such literals, but the artifact was built without `UNAOS_IVB3D_R9` (added to the line by GEN7R9B, after this tree). `[gen7] r9 ring=rcs`: 0 | none: stays `unmeasured-here`; measure on the first build with `UNAOS_IVB3D_R9=1` |
| stria | `:: STRIAAPP: image=` | 0 | STRIAMETAL folded after this tree. `STRIA.ELF`: 0 | none: stays `unmeasured-here` |
| midden | `:: MIDDENAPP: image=` | **1** | present | `midden\|:: MIDDENAPP: image=\|@arch:x86_64\|measured(1)` |
| una | `:: UNAAPP: image=` | **1** | present | `una\|:: UNAAPP: image=\|@arch:x86_64\|measured(1)` |
| sonde | `:: SONDE:` | **5** | present | `sonde\|:: SONDE:\|@arch:x86_64\|measured(5)` |
| installdemo | `:: INSTALLVERB: census disk=` | 0 | already `measured(4)` in the table; this artifact was built without the installer knobs (the source at `0d88e0c9e` has the literal in `shell.rs`) | none: the row is already measured on another artifact |
| ahciroot | `:: AHCIROOT: grant=` | 0 | already `measured(9)`; the knob was not armed here (`install/ahciroot.rs` has 10 sites) | none |
| ahci-write | `:: AHCI: write path ARMED` | 0 | already `measured(1)`; not armed here | none |
| instgui | `[wc-x] instgui DECLINE reason=create-failed` | 0 | already `measured(1)`; not armed here | none |
| orindesk | `[orinwm1] DECLINE reason=no-panel` | 0 | `@arch:aarch64`: an x86 artifact cannot carry it | none: stays `unmeasured-here`, needs an Orin (aarch64) artifact |
| orinclick | `[orinclick] arm panel=` | 0 | `@arch:aarch64` | same |
| orinconwin | `[orinconwin] DECLINE reason=no-panel` | 0 | `@arch:aarch64` | same |
| orinrender | `[render] arm conwin=` | 0 | `@arch:aarch64` | same |
| tegra_el0 | `:: TEGRA-EL0: el0-hello round-trip` | 0 | `@arch:aarch64` | same |

`wedge2` and `prefs_reset` have no witness literal by design (their rows say so), so they cannot be grepped.

## Files

- Run logs (scratch): `~/unaos-bench/scratch/rmbp-0915/local3/knoboff27/{wc-6f13,NOTIFYCENTER,…,STRIAMETAL}.log`, `all.out`,
  `bcgrep.txt`, and the drivers `run0.sh`, `run1.sh`, `all.sh`, `bcgrep.sh`.
- The knoboff caller directory (`~/unaos-bench/scratch/knoboff/caller-agent-a4d87bde9eb46c198-f44071486808/`: knoboff-tree, target,
  and the five kept run directories) was **deleted** after the runs, as the brief said. Its worktree registration
  (`knoboff-tree2`) was pruned. The image digests above are the record.
