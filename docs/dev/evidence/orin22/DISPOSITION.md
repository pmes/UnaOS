# orin 22 · DISPOSE — per-commit disposition of the render10 stack

Base: `hw-jetson` **98213b7f**. Tip: `exec-orin21-tearfold` **45870e80**.
Classification is derived from the diffs (`git show`), not from commit messages.
Also on origin, off 98213b7f: `exec-orin21-ledger` fc1a2d52, `exec-orin21-lawsnom` f86cf448 (not in scope below).

Legend — **A** = direction-violating (binds root/boot/apps by board or slot name; compares "the slot card"
to the boot medium; embeds a card serial/geometry/label as a const or assertion; adds a board-named knob or
leg; regrows/reformats a specific card). **B** = instrument/generic. **C** = mixed.

## The table

| # | sha | files (diffstat) | class | evidence — declaration sites at that commit |
|---|-----|------------------|-------|---------------------------------------------|
| 1 | **76bf3267** TEARSCOPE | `video/dock.rs` 11, `video/menubar.rs` 15, `video/strip.rs` 517 | **B** | Pure video instrument. `strip::paint`'s `return false` arms become `bar_decline(name, DECL_*)`; `erase_rect` call sites become `strip::vacate(tenant, …)`. No medium, no block layer. |
| 2 | **74b7f21b** UNAFSGROW | `arroyo` 247, `scripts/make-pi-img.sh` 38, `specs/jetson-sync1.spec` 88 | **C — mostly A** | **A:** `arroyo:5239-5240` `ORIN_CARD_IMG=…/UnaOS-orin-card.img`, `ORIN_UNAFS_IMG=…/unafs-orin.img`; `arroyo:5241` `esp_jetson_img()` — a verb that *regrows and repartitions the Orin's card* (8 MiB tail → 512 MiB), `arroyo:5242` `UNAOS_ORIN_UNAFS_MB:-512`, `arroyo:5272` `UNAOS_IMG_FAT_LABEL=UNAOS-ORIN`, and the readback assertion block `arroyo:5325-5326` `if label != "UNAOS-ORIN"` plus the pinned `p1 start != 2048` / `p2 count != uwant*2048` fails. `arroyo:7444` registers the `esp-jetson-img` verb. Spec side: `jetson-sync1.spec:405` `PENDING PART: mbr handle=tegra-sd slot=2 … start=262144 count=1048576 end=1310720`, `:406` `PENDING part=\[262144\.\.1310720\) … span_blocks=131072 …`, `:413` `PENDING TEGRA-UNAFS: native unafs volume MOUNTED read-only on TegraSd`, `:436` `FORBID span_blocks=2048 fits=` — the card's geometry pinned as a rule. **B:** `make-pi-img.sh:160-161` `UNAFS_MB="${UNAOS_IMG_UNAFS_MB:-8}"` / `FAT_LABEL="${UNAOS_IMG_FAT_LABEL:-UNAOS-PI}"` — two baked literals become knobs, defaults byte-for-byte today's values; board-agnostic. |
| 3 | **34a3d2f5** MENUOWN | `video/menubar.rs` 42, `video/winmenu.rs` 70 | **B** | `menurow_witness(&model)` + `static MENUROW_KEY` — edge-triggered `[menubar] menus cap_owner= … items=` line. Video only. |
| 4 | **2016dc41** MENUOWN | `video/menubar.rs` 88, `video/winmenu.rs` 21 | **B** | Menu owner reduction changes from "frontmost visible publisher" to the caption's row. Video only. |
| 5 | **45269245** MENUOWN | `video/menubar.rs` 39, `video/winmenu.rs` 30 | **B** | Menu titles follow the app title instead of an absolute column. Video only. |
| 6 | **c59f8ddb** ledger | `docs/dev/OS/orin-ledger.md` 4 (2 rows) | **B** | Two ledger rows corrected (A25 wording, A6 gets a fetchable sha). Doc. |
| 7 | **c1f924d6** UNAFSROOT | `sdmmc_tegra.rs` 219, `docs/…/orin-unafs-root.md` 109, `jetson-sync1.spec` 38 | **C — mostly A** | **A:** `sdmmc_tegra.rs:3456-3458` `fn root_native_witness() { … locate_on(BlockHandle::TegraSd) }` — root is chosen by *the Tegra SD slot*, the exact shape Peter's ruling deletes; `:3370` `const ROOT_NATIVE: u8 = 3`; `:3493` `return ROOT_NATIVE` in `root_probe`; `:3587` `if verdict != ROOT_NATIVE { mt.mount("/", FatBackend::new_tegra_sd(…)) }` — `/`,`/boot`,`/apps` bound by slot name. Whole change lives under `#[cfg(feature = "sdmmcroot")]` (the mechanism BOOTROOT is deleting). **B-ish:** `jetson-sync1.spec:389` `FORBID unafs has no volume here` — a tripwire against an *asserted absence*, generic in shape but it guards only the `sdmmcroot` emitter. Doc `§5.1` is the design record of the dead direction. |
| 8 | **c7295870** ORIN-SPECARM | `arch_arm64.md` 79, `orin-ledger.md` 2, `orin-specscore.py` 51, `jetson-sync1-green.capture` 40, `jetson-sync1.spec` 301 | **B** | Nine tick/preempt rows PENDING/OPTIONAL → REQUIRE/COUNT/FORBID; scorer gains a FAILABILITY line (`decl = [d for d in directives if not d.builtin]`, reporting-only, touches no exit code). Nothing about media, root or cards. |
| 9 | **3cc13a80** EVQPRINT | `display_tegra.rs` 56, `green.capture` 19, `jetson-sync1.spec` 74 | **B** | `[ptrpoll]` line gains `folded=`/`foldnew=` from `pal::pointer_motion_coalesced()`; new `PTRPOLL_FOLD_LAST` static at EOF. Pointer instrument. |
| 10 | **5c1c242a** INTEGRATE | `green.capture` 7 | **B** | Header FORBID tally re-derived from `orin-specscore.py`'s FAILABILITY line instead of carried by hand. |
| 11 | **c50b82c6** shell verdict | `shell.rs` 28 | **A** | Rewrites the `sdmmc_root_bind` tail comment at `shell.rs:7180` and the `vfs_mount_table` doc at `:7144-7150` to document `ROOT_NATIVE`/`ROOT_BOUND`/`ROOT_REFUSED` — i.e. root chosen by the Tegra slot walk. Also `shell.rs:3696` `vfsroute_witness` comment asserting the Orin now takes the Pi's two-volume shape. Comment-only, but it is the dead direction's documentation, in a BOOTROOT file. |
| 12 | **72e2ecff** ORIN-BOOTID | `sdmmc_tegra.rs` 2, `drivers/block.rs` 8, `main.rs` 2 | **C** | **A:** `sdmmc_tegra.rs:3507` — folded onto `let label = fs.label();`, compares `boot_volume_serial()` against the **Tegra-slot FAT's** `vol_id` and `return ROOT_REFUSED`. This is "compare the slot card to the boot medium", the shape Peter names. **B / salvageable:** `block.rs:942/978/1001` widen `BOOT_VOLUME_SERIAL`, `set_boot_volume_serial`, `boot_volume_serial` from `all(x86_64, sdhcblk)` to `any(all(x86_64,sdhcblk), all(aarch64,sdmmc))`; `main.rs:2311` publishes `boot_info.boot_volume_serial` on the tegra `tegra_early_stop` path. **Those three lines are the loader's FAT serial reaching aarch64 at all — the input Peter's ruling requires.** They are gated on `sdmmc`, not `sdmmcroot`, and are board-agnostic. |
| 13 | **96f686be** WITNESSGAP | `docs/dev/LAWS.md` 23, `arroyo` 113 | **B** | Four witness-free gate legs (`arm-tegra-bsprun-nowitness`, `…-el0-nowitness`, `arm-pi-nowitness`, `x86-all-nowitness`) + a witness-polarity census printed by `check_kernel_cfg`. Build-gate coverage; no medium content. |
| 14 | **d1766401** PANICLOC | `scripts/panicloc-normalize.py` +486 (new) | **B** | Host script: hashes a kernel image with `panic::Location` line fields normalized out. Platform-agnostic. |
| 15 | **2f391c22** PANICLOC | same file, 17 | **B** | Aligns the scan on vaddr; verdict wording. |
| 16 | **0f06d60c** DOCTRUTH | `docs/…/orin-unafs-root.md` 113 | **C — A-adjacent** | The *retraction discipline* is generic ("a correction that does not retract is not a correction"); the document it corrects is the `sdmmcroot`/GPT-card-layout design of record, and the corrections are card facts (MBR vs GPT, `label="UNAOS-PI" vol_id=0xabfbdefa`, the 8 MiB-partition/4 MiB-volume split). Dies with §5.1. |
| 17 | **aec2c604** BOOTIDLIVE | `sdmmc_tegra.rs` 252 | **A** | `sdmmc_tegra.rs:3470` `enum BootMedium`; `:3494` `const fn boot_medium_verdict(boot_serial: u32, card_esp_serial: Option<u32>) -> BootMedium`; **`:3534` `const RENDER9_LOADER_SERIAL: u32 = 0xde00_1a13;`**, **`:3538` `const RENDER9_CARD_ESP_VOL_ID: u32 = 0xabfb_defa;`**, **`:3545` `const RENDER10_STAGED_ESP_VOL_ID: u32 = 0xde00_1a13;`** — three literal card serials compiled into the kernel; `:3554` `const _: () = { assert!(matches!(boot_medium_verdict(RENDER9_LOADER_SERIAL, Some(RENDER9_CARD_ESP_VOL_ID)), BootMedium::Foreign)); … }` — the serials asserted at compile time. `:3671` `let card_esp = fat.as_ref().ok().map(\|fs\| fs.volume_fingerprint().0)` from `BlockSource::TegraSd`; `:3678` `BootMedium::Foreign => … return ROOT_REFUSED`. Textbook "embeds a card serial as a const or assertion" plus "compares the slot card to the boot medium". |
| 18 | **086ec38d** fitsland | `partitions.md` 47, `fs/unafs.rs` 126 | **B** | `fn span_fit_report(p, span, sb0, magic_ok)` appended at EOF of the arch-neutral `fs/unafs.rs`; `fits=` stops being the tautology `(n/8)*8 <= n` and compares the **superblock's own** `block_count`; adds `sb_blocks=` and an overrun `=> FAIL ::` line. No board, no slot, no serial. |
| 19 | **b8b7f90c** fold | merge (aec2c604 + 086ec38d) | — | No unique content; equals 086ec38d's diff against aec2c604. |
| 20 | **21727dc0** fold | merge (b8b7f90c + 74b7f21b) | — | No unique content; equals 74b7f21b (minus 1 spec line). |
| 21 | **6cf9f13b** ORINFOLD | `jetson-sync1.spec` 26 | **A (by dependency)** | Re-keys UNAFSGROW's `:406` PENDING row to `… span_blocks=131072 sb_blocks=131072 fits=yes magic=ok` and the FORBID to `span_blocks=2048 sb_blocks=`. Both rows are #2's card-geometry pins; there is nothing here without them. |
| 22 | **3894fdc9** REKEY | `jetson-sync1.spec` 16 | **C** | **A:** `FORBID span_blocks=2048\b` — same card-geometry pin, one field instead of a junction. **B:** the second hunk is a comment-only note on the pre-existing `FORBID Exception reason=1 syndrome=0x82000010` row explaining why that literal stays whole (EL3 firmware emitter). Salvageable as a comment. |
| 23 | **4a61071f** merge | merge (3894fdc9 + 76bf3267) | — | No unique content. |
| 24 | **45870e80** strip:820 | `video/strip.rs` 4 (2 lines) | **B** | Corrects a comment claim about what boot 37's `[wc-h] rollup scope=window torn=` actually read. Video comment. |

## Cherry-pick test onto 98213b7f (`git cherry-pick --no-commit`, throwaway worktree, nothing committed)

**The whole (B) set applies clean, in this order, with zero conflicts:**

`76bf3267 · 45870e80 · 34a3d2f5 · 2016dc41 · 45269245 · c59f8ddb · c7295870 · 3cc13a80 · 5c1c242a · 96f686be · d1766401 · 2f391c22 · 086ec38d`

Result: 15 files, +2109 / −155 — `LAWS.md`, `arch_arm64.md`, `partitions.md`, `orin-ledger.md`, `arroyo`,
`display_tegra.rs`, `fs/unafs.rs`, `video/{dock,menubar,strip,winmenu}.rs`, `orin-specscore.py`,
`panicloc-normalize.py`, `jetson-sync1{-green.capture,.spec}`. **No (A) content is carried in.**

Singly, these need a predecessor and conflict alone (all resolved by taking the stack order above):
`45870e80` (needs 76bf3267, `strip.rs`), `c7295870` (needs c59f8ddb, `orin-ledger.md`),
`5c1c242a` (needs c7295870+3cc13a80, `jetson-sync1-green.capture`), `2f391c22` (needs d1766401,
`panicloc-normalize.py` modify/delete).

Not cleanly pickable without (A): `6cf9f13b` and `3894fdc9` — both conflict in
`unaos/scripts/specs/jetson-sync1.spec` because the rows they re-key are UNAFSGROW's card-geometry
pins, absent from 98213b7f. 3894fdc9's `Exception reason=` comment hunk can be re-typed by hand.

Salvage, verified separately: `git diff 98213b7f 74b7f21b -- unaos/scripts/make-pi-img.sh` applies
clean on its own (the `UNAOS_IMG_UNAFS_MB` / `UNAOS_IMG_FAT_LABEL` knobs, defaults unchanged).

## Overlap with executor BOOTROOT (`exec-orin22-bootroot`, deleting `sdmmcroot`)

- `shell.rs`, `fs/vfs.rs`, `sdmmc_tegra.rs`: **no (B) commit touches them.** Only (A) commits do
  (c1f924d6, c50b82c6, 72e2ecff, aec2c604) — all dead by direction, so no contention.
- `unaos/arroyo`: **96f686be (B) is the one to watch.** Its four `KERNEL_CFG_MATRIX` rows are inserted
  immediately after the `"arm-tegra-sdmmcroot …"` leg line, which BOOTROOT deletes — adjacent-line
  conflict is likely. Trivial to resolve (keep the four new rows, drop the deleted leg). Also
  74b7f21b's `esp_jetson_img` (A) lands in `arroyo`; dropping it removes the collision entirely.
- `specs/jetson-sync1.spec`: c7295870 + 3cc13a80 (B) both edit it; c1f924d6's `FORBID unafs has no
  volume here` and UNAFSGROW's four card rows (A) are the lines BOOTROOT will want gone. The (B)
  edits are in the tick/preempt and `[ptrpoll]` blocks — disjoint from the sdmmcroot block — so a
  conflict here is unlikely, but both seats write the file.

## Summary (≤10 lines)

1. **Clean to keep, one 13-commit stack, no conflicts:** TEARSCOPE (76bf3267, 45870e80), MENUOWN
   (34a3d2f5, 2016dc41, 45269245), ledger (c59f8ddb), ORIN-SPECARM + EVQPRINT + INTEGRATE
   (c7295870, 3cc13a80, 5c1c242a), WITNESSGAP (96f686be), PANICLOC (d1766401, 2f391c22),
   fitsland (086ec38d). ~2100 lines of instrument, gate and doc with no medium binding in them.
2. **Dead by direction:** c1f924d6 UNAFSROOT, aec2c604 BOOTIDLIVE, c50b82c6, 6cf9f13b — root chosen
   by `locate_on(TegraSd)`, three literal card serials (`RENDER9_LOADER_SERIAL`,
   `RENDER9_CARD_ESP_VOL_ID`, `RENDER10_STAGED_ESP_VOL_ID`) asserted at compile time, and the card's
   MBR geometry pinned as spec rows. All under `sdmmcroot`, which BOOTROOT is deleting anyway.
3. **Mixed, needs a split:** 74b7f21b (drop `esp_jetson_img` + the four spec rows; **keep**
   `make-pi-img.sh`'s two knobs, verified applying alone), 72e2ecff (drop the `sdmmc_tegra.rs:3507`
   comparison; **keep** the `block.rs` cfg widening + `main.rs:2311` publish — that is the loader FAT
   serial reaching aarch64, the one input Peter's direction actually needs), 3894fdc9 (drop the
   `span_blocks` FORBID; the `Exception reason=` comment is re-typable), 0f06d60c (the
   retract-don't-accrete lesson belongs in LAWS; the document it corrects dies with §5.1).
