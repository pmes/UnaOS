# ORIN 22 — BULLETIN (live)

## 0. OPENING (2026-09-08 14:10Z, verified, not relayed)
- Host unfiltered ls-remote: hw-jetson 98213b7f · hw-pi4 059e04db · hw-rmbp d9ec435f · main c7407753 ·
  exec-orin21-{fold 6cf9f13b, rekey 3894fdc9, tearfold 45870e80, ledger fc1a2d52, lawsnom f86cf448} all on origin.
  Nothing from orin 21 owed a push. Local hw-jetson == origin, clean.
- Card in: mmcblk0p1 UNAOS-ORIN (render9). Butler holds the port (pid 81753).
- Peers: rmbp 16 local_1c3dce31…, pi 9 local_aee72caf…. Both messaged this turn (grant ask to rmbp; heads-up to pi).

## 1. THE DIRECTION (Peter, this morning, verbatim gist)
"It is an OS booting off an SD card. The card is the hard drive. Every boot is stone cold — no prefs, no
special checks. Drivers boot cold, boot dumb, presume nothing about the machine even though we keep booting
the same machine." → root = the volume the loader was loaded from, found by its FAT serial among the disks the
kernel enumerates. No knob, no board, no fallback to a table of volumes the machine does not have.

## 2. FACTS THE ARC STANDS ON (verified in tree at 98213b7f)
- Loader already passes `BootInfo::boot_volume_serial` (bootloader/src/main.rs:193); render9 wire: `boot volume FAT serial 0xde001a13`.
- Kernel consults it only on x86 (`drivers/block.rs:342 program_source`, cfg x86_64+sdhcblk); main.rs:263 publish is x86-gated.
- On the Orin the `sdmmcroot` bind rooted on the slot card `vol_id=0xabfbdefa` (label UNAOS-PI) — a different volume. The wrong disk.
- D2/D3 (guard, RENDER9_*/RENDER10_* consts) are NOT on hw-jetson; they live on exec-orin20-bootidlive aec2c604 and its descendants (fold/tearfold). D1/D4/D5 are on hw-jetson.

## 3. FLEET
| name | branch | base | ask |
|---|---|---|---|
| BOOTROOT | exec-orin22-bootroot | 98213b7f | the dumb root bind + delete sdmmcroot/new_tegra_sd/knob/leg |

## PUSHES PETER WILL NEED (batched at minute one)
```
git push origin exec-orin22-bootroot
```
At close: `hw-jetson` if the arc lands there.
| STAGE11 | (bench, scratch/orin22/stage11) | — | render11 flight doc + scorer11 keyed on `[vfs] root = boot volume`, mutation-proven, no hardcoded serials/geometry |
| DISPOSE | (read-only, scratch/orin22/dispose) | — | per-commit disposition of the render10 tree 98213b7f..45870e80: direction-violating vs instrument/generic, cherry-pick test onto 98213b7f |

## 4. SUPERSEDED: the staged render10 candidate (scratch/orin21/tearfold/render10-candidate/) carries the
boot-medium guard + this bench's card serials as consts (aec2c604). It does not fly. render11 = BOOTROOT's tree.
All three executors on Opus (`model:"opus"` explicit). Grant ask to rmbp 16 pending (shared kernel-core files).

## 5. PEERS (14:35Z–14:45Z, verified in my tree)
- rmbp 16 GRANTED all five shared files (block.rs, fat.rs, vfs.rs, shell.rs, main.rs). Three conditions adopted
  into the brief: main.rs:258-262 comment rewritten same commit; one witness line with a reason field; six stranded
  prose sites fixed same commit. B60 C1 dissolves with the bind — off the owed list. x86 arm of vfs_mount_table and
  program_source LEFT ALONE this arc (their harness: boot ESP is a separate ide-hd by design).
- pi 9 WITHDREW its "guard the Pi" ask: a Pi branch is the knob. Peter (14:45Z): the Pi/Orin card difference
  "SHOULD NOT MATTER IN EITHER CASE". → ONE body for aarch64; Pi bare-metal (boot.rs:1169 serial 0) lands on the
  NONE arm; kernel8-test rows assuming `/` will red — BOOTROOT lists them, edits pi4-regression.spec only on a
  PI-GRANT.md from pi 9 (asked).
- BOOTROOT v1 KILLED at ~4 min (no channel to amend a running executor: SendMessage disabled in this session).
  Branch/worktree removed clean; its two baseline logs kept (QEMU aarch64 loader passes serial 0xfabe1afd).
  BOOTROOT v2 spawned on the merged brief (Opus). Lesson: the brief is the only channel — get it right before spawn.
- rmbp 16 (14:55Z): scoping CONFIRMED by them — no AHCI/SATA/IDE driver in the tree; the x86 QEMU harness's
  separate ide-hd ESP is a harness choice, metal (card in a USB reader = Default) already matches the dumb walk.
  x86 harness fix + convergence QUEUED ON rmbp, not asked of orin. `fat::locate_boot_volume` is the shared seam.
- pi 9 (15:00Z) GRANTED option (a): BOOTROOT re-keys the measured red rows in pi4-regression.spec, one commit,
  six conditions (measured list; re-key only, no deletions; floor arithmetic 120=118 REQUIRE+2 COUNT in the
  message; ADD one REQUIRE for `[vfs] root -> NONE reason=…` → floor 121; bounding rules; list before commit).
  Written to scratch/orin22/bootroot/PI-GRANT.md — the channel the brief names.

## 6. MECHANISM CHANGE (15:05Z) — Peter to pi 9: "WTF does it matter what method I choose to boot? You are assuming too much."
A loader hand-off makes the boot method load-bearing (UEFI yes, Pi firmware no). AND DISPOSE found main.rs:263's
serial publish is DEAD on a tegra image (`kernel_main` → `tegra_early_stop -> !` at :190; 72e2ecff had folded a
second publish into tegra_early_stop). Two defects in v2's brief → v2 STOPPED (survey + 2-file un-gate patch kept).
**v3 mechanism: the kernel finds the disk that has THIS KERNEL on it, by content** (window of its own running image
vs candidate files' bytes; ELF PT_LOAD at p_offset / flat at 0) over every enumerated block source's FAT volume.
Then the OS layout over that disk: /boot = that FAT volume; / = the disk's UnaFS if present else /boot; /apps = /boot
rooted APPS/. Pi: kernel8.img on FAT p1 + UnaFS p2 on the same card → today's table, floor unchanged (expected).
pi 9 WITHDREW grant condition 4 (no REQUIRE for the NONE line; floor stays 120). rmbp 16 informed; grant unchanged.
- DISPOSE DONE: scratch/orin22/dispose/DISPOSITION.md — 13 generic commits cherry-pick clean onto 98213b7f
  (TEARSCOPE+strip820, MENUOWN×3, ledger, SPECARM, EVQPRINT, INTEGRATE, WITNESSGAP, PANICLOC×2, fitsland);
  dead by direction: c1f924d6 UNAFSROOT, aec2c604 BOOTIDLIVE, c50b82c6, 6cf9f13b; mixed: 74b7f21b (keep only
  make-pi-img.sh knobs), 72e2ecff (keep block.rs widen + tegra publish — now in BOOTROOT v3), 3894fdc9, 0f06d60c.
- KEEP13 spawned: `exec-orin22-keep13` = the 13-commit stack, full gate incl. UNAOS_WC=1 test.
FLEET: BOOTROOT v3 · STAGE11 · KEEP13 (Opus). PUSHES: `git push origin exec-orin22-bootroot exec-orin22-keep13`

## 7. (15:15Z) rmbp 16 BLOCKER accepted: TegraSd variant exists only under `sdmmc`; esp_jetson never forces it →
plain image cannot see the slot. `sdmmc` becomes default-on in esp_jetson (opt-out UNAOS_NOSDMMC=1) IN THIS ARC.
Wire correction to rmbp's premise: render9 loader serial ≠ slot card; PSRC global=present → boot medium on USB
that flight. pi 9: PI-GRANT cond.4 withdrawn (file fixed 15:12Z); Pi image = FAT p1 kernel8.img + UnaFS p2
unconditional in kernel8() → floor EXPECTED 120; window must be .text (BSS zeroed on Pi). rmbp: multi-match
must be counted and refused, one-byte flip mutation required. → BOOTROOT v3 STOPPED (still in setup), v4 spawning
with amendments 01+02 folded. Three stops today; cause each time: the brief is the only channel into an executor.
LAWS nomination (pi 9): "require a PROPERTY, never a LIMITATION — a required limitation makes the gate the defect's advocate."
- (15:25Z) BOOTROOT v4 SPAWNED (Opus): amendments 01+02 folded; sdmmc default-on in esp_jetson (opt-out
  UNAOS_NOSDMMC=1); .text window named at the site; counted multi-match → REFUSE; three mutations m1/m2/m3;
  v2-ungate.patch reused. FLEET: BOOTROOT v4 · STAGE11 · KEEP13.
- (15:30Z) rmbp 16: premise withdrawn (wire beats comment). QUEUED for the seat after v4 reports (same files):
  (a) repair `sdmmc_tegra.rs:36-37` "the bootloader read the card to boot" — misled a seat within an hour;
  (b) `#require=sdmmc` in the staging predicate (KNOBS env / STAGE11's FLIGHT-render11.md) is VACUOUS once sdmmc
  is default-on — staging must assert the `[sdmmc]` census witness by `strings` on the artifact and the ABSENCE
  of UNAOS_NOSDMMC in the banner, not a knob; (c) UNAOS_NOSDMMC is a negative knob = the UNAOS_NOTEGRASMP shape
  (conditional add in esp_jetson, not a subtraction) — GATE-K8REACH universe is rmbp's to widen.
  rmbp verified: zero FORBIDs keyed on sdmmc/tegra-sd witnesses in jetson-jd5/jetson-sync1 → default flip cannot false-red.
- (15:40Z) rmbp 16: GATE-K8REACH reds an unregistered `_feats` knob → UNAOS_NOSDMMC needs a `k8-reach.registry`
  row. VERIFIED HERE: `unaos/scripts/k8-reach.{py,registry}` do NOT exist at hw-jetson 98213b7f (nor on main);
  they are hw-rmbp-only. Not a gate at my tip; becomes one at trunk-sync/landing → LANDING CHECKLIST item: add
  the UNAOS_NOSDMMC registry row (NOTEGRASMP shape) in the merge that brings the gate and the knob together.

## 8. (15:50Z) STAGE11 DONE — scratch/orin22/stage11/: scorer11.sh (6 legs, exits 0/1/2/3), mutate11.sh, 21-row
mutation matrix (every leg ≥4 outcomes), FLIGHT-render11.md, positive control = `crates/kernel/src/` (32/47/53 on
three real kernel.elf builds; KELF/ORIN-CARD were the bad controls). No card operation proposed; write is Peter's
(`load-card10.sh --target /dev/mmcblk0 --allow-geom-absent`, host sudo). Butler pid 81753 confirmed.
SEAT RE-KEY (STAGE11 was cut on the v2 contract): NONE reasons → v4 vocabulary (no-disk-enumerated |
kernel-not-found-on-any-volume | walk-cap-hit | multiple-kernels); matrix re-run: identical outcome shape, M05/M06
FAIL on the new reasons, M12 undocumented FAIL (out/matrix-v4.txt). FLIGHT §1/§2 amended: `#require=sdmmc` vacuous
once default-on; staging asserts banner `sdmmc`, no NOSDMMC in env, `[sdmmc]` census ≥1 by strings. Pre-edit
copies: scorer11.sh.pre-v4, FLIGHT-render11.md.pre-v4.
- LAWSNOM2 spawned (docs-only, NO battery — A5): five bullets (boot dumb; property-not-limitation; the brief is the
  only channel; the terminus kills what is below it; wire beats comment) on `exec-orin22-lawsnom2`.
FLEET: BOOTROOT v4 · KEEP13 · LAWSNOM2. PUSHES: `git push origin exec-orin22-bootroot exec-orin22-keep13 exec-orin22-lawsnom2`
- (16:00Z) PETER'S SCENARIO → AMENDMENT 03 (bootroot/BRIEF-AMENDMENT-03.md): multi-match counted per DISK; tiebreak
  = the loader's reported volume when it exists, else REFUSE; every non-root disk mounted read-only at /usb or /sd
  (bus-named) for quarry. Applied as a FOLLOW-UP commit after v4 reports (no 4th restart).
- (16:10Z) rmbp 16 (their census, `git cat-file -e` per file per ref): SEVEN gates exist ONLY on hw-rmbp
  (k8-reach.py/.registry, k8-modtree.py, knob-leg-covered.py, check-roots.sh, arch-families.sh, append-position.sh);
  ELEVEN diverge, incl. ledger-check.sh +6/−190 (rmbp 12's four mutation-proven fixes not in my tree) and mbench.py
  (2/10; the `.strip()` parser fact identical). FOR PETER (rmbp's framing, attributed): J1 unlanded = its gates
  protect nobody and its fixes reach nobody; the price is not review size. UNAOS_NOSDMMC registry row = rmbp's at
  J1 landing (orin lands first). ACCEPTED rmbp's offer: their ledger-check over my ledgers at 98213b7f.
- (16:15Z) pi 9: `/usb` is WRITABLE on the Pi today (vfs.rs read_only()=false for Usb — verified BOT WRITE(10));
  `world_readable` is a READ posture, not write protection; no pi spec row exercises /usb (blind, not preserved);
  the USB write path has ZERO gate coverage → pi's queue. AMENDMENT 03 corrected: non-root disks mount with the
  SOURCE'S OWN posture (Usb rw, TegraSd vetoed → /sd ro by the veto). Verified at 98213b7f fat.rs write_veto arms.
- (16:25Z) PETER: "booting dumb means booting dumb. If it sees another UnaOS disk it is home soil and nothing more."
  → AMENDMENT 03 v2: no multi-match refusal, no loader-serial tie-break; first found = root, the rest mounted; dedupe
  by DEVICE (block.rs:705-709 publishes one USB device as both Default and Usb on tegra — rmbp's D confirmed);
  indexed bus mount points. rmbp's A/B (ambiguous-serial refusal; serial is refusal-direction only) are MOOT: the
  serial is not in root's decision at all.
- (16:40Z) rmbp 16 ran THEIR ledger-check over MY ledgers at 98213b7f (throwaway worktree): OK 125 rows, exit 0
  both gates. Differential proven by mutation with both controls: my gate is BLIND to prefixed cross-refs
  (`→ SO1…SO7`, `→ SO14`: 7 today, all resolve) — silent on `→ SP999` while its summary says "cross-refs resolve";
  positive control (bad sha) reds in both. Nothing to repair; coverage arrives when their gate reaches trunk.

## 9. (16:45Z) LAWSNOM2 DONE — `66523c05` on exec-orin22-lawsnom2 (docs/dev/LAWS.md +134/−0; verified `git log -1`).
Five bullets (Direction: boot dumb · Verification: property-not-limitation, terminus kills what is below ·
Arcs: the brief is the only channel · Coordination: wire beats comment). No f86cf448 duplication. GATE-LEDGER
exit 0 but it does NOT score LAWS.md (no script references it) — read-back was the check. No battery (A5).
- (16:50Z) BUILDPERF-REPORT spawned (bench-side, no runs): writes orin 21's unwritten report from data on disk —
  the evidence for Peter's open decision B8 (kernel8-test fast-mode shape). FLEET: BOOTROOT v4 · KEEP13 · BUILDPERF-REPORT.
PUSHES: `git push origin exec-orin22-bootroot exec-orin22-keep13 exec-orin22-lawsnom2`
- (16:55Z) PETER (to rmbp 16, verbatim): "dumb is not the right word but not making assumptions and not tying them
  together possibly staining the testing of the newer version." → the property is ISOLATION between versions.
  rmbp 16: a 4 KiB .text window can be identical across two builds → a newer kernel could root on the older
  install's disk (first-found). AMENDMENT 03 item 5': the compared bytes include a .text build stamp carrying
  UNAOS_GIT_SHA (arroyo:50 exports it; option_env! at genet.rs:2572 is its only reader today); witness prints
  `sha=`; m7 = two commits on two disks each boot to their own. Limit at the site: dirty builds of one commit share
  a stamp (no per-build nonce — same-commit byte identity is load-bearing for the identity gates).

## 10. (17:05Z) KEEP13 DONE — `exec-orin22-keep13` tip `51f06bea` (verified `git log -1`): 13/13 picks with -x, zero
conflicts, 15 files +2109/−155 = DISPOSITION's prediction exactly; 0 added-line hits of any direction token.
FULL GATE GREEN: check 0 (60 legs, 0 ❌; witness census ON 51/OFF 9) · UNAOS_WC=1 test 0 (banner `…,wc`; DOCK PASS,
WINMENU PASS) · crystal `menu_x=0 glyph_x=12 anchor=panel-left` exact · test-arm 0 · kernel8-test 300 125/125 ·
esp-jetson 0. Landing flag: df64ef5f (WITNESSGAP) inserts 4 matrix rows directly below the `arm-tegra-sdmmcroot`
leg BOOTROOT deletes → adjacent-line conflict expected; resolution = keep the 4 rows, drop the leg.
- (17:10Z) PETER (to rmbp, verbatim): "seeing another UnaOS disk is like seeing a friend — nobody wants another to
  wrap their life around them just because they're acquainted." rmbp B90 → render11 leg FRIEND-DIFF (FLIGHT §6):
  two boots, reader card in/out, normalized wire diff modulo mount witnesses must be EMPTY; survivors = named
  entanglements. Normalizer written with the first wire pair (timing fields).
- (17:20Z) rmbp 16 SECOND-EYES on KEEP13 `51f06bea`: PASS by their own greps — strip.rs:820-821 verbatim, `scope=bar`=4,
  and the CONTROL I did not run: `read torn=0 all boot` → 0 (false sentence REMOVED, not coexisting). RULE (rmbp,
  adopted): every cross-lane cherry-pick uses `-x` — dangling citations resolve by `git log --grep=<orig sha>`.
  Their render11 stake: the `owed=yes/no` baseline leg + FRIEND-DIFF only.
- (17:25Z) rmbp 16 on FRIEND-DIFF: a normalizer built from the judged pair greens itself. FLIGHT §6 corrected:
  three boots (absent, absent, present); normalizer from the same-condition pair, frozen; positive control =
  prtscr's rung-2 capture to the friend under a read-only boot volume (must RED).

## 11. (17:35Z) PETER, verbatim: "another way to think of it is like on the macbook if UnaOS saw catalina and
## immediately formatted the disk as an alien enemy."
THE RULE IN THREE CLAUSES: the disk with THIS kernel on it is home · another UnaOS disk is a friend you do not wrap
your life around · any other disk is a STRANGER you do not touch. The kernel never writes, formats, or installs onto
a disk on its own initiative; a disk changes only by an operator's explicit act.
EXISTING ALIEN-ENEMY SHAPES (knob-gated, absent from the plain image, NOT touched by this arc, flagged for the next
arc near them): `sdmmc_install_from_usb` (main.rs ~2520, `install_target`: formats the slot card at boot and clones
the running system onto it) · `selfup_service` (`selfup`: rewrites media at boot) · soft form: prtscr's capture
ladder falling to another disk when the boot volume refuses writes (rmbp's FRIEND-DIFF positive control).
LAWS: the "boot dumb" bullet (66523c05) gets this third clause at landing (line-neutral append, cite this §).
- (17:40Z) rmbp 16 self-correction: arroyo diverges 603/548 between trees; their ":54" was their coordinates, mine
  ":50" was right for my tree. Rule both seats now hold: cross-tree citations name the MARKER (`BUILD-SHA-1`), never
  the number. No change on my side.

## 12. (17:50Z) BUILDPERF REPORT WRITTEN from orin 21's data — scratch/orin21/buildperf/REPORT.md (120 lines, no runs).
check: cold 736.6 s / warm 156.9 s; serial cfg-matrix share 75.6% cold / 98.8% warm (56 legs, one core of 20).
kernel8-test 300: last COMPLETE at +11.0/+45.3/+13.7 s of a 299.1 s span → idle tail 85–96% (three PASS runs).
Parallel legs w/ per-slot target dirs: 177.5 s cold / 1.9 s warm vs 557.2/155.0 serial — the warm matrix is
target-dir THRASH, not work (2.5 GB for 6 slots). Not on disk: k8t-cold tail (cut at T0), sccache/mold (not installed).
Open: ✅-line count 76 on disk vs BULLETIN 77; FORBID count orin 135+3 vs pi 134+3. FOR PETER: decision B8.
FLEET: BOOTROOT v4 only (follow-up + comment repair + keep13 merge all queued behind its report).
- (17:55Z) FRIEND-DIFF control = option 1 (durable one-line mutation per run); prtscr rung-2 is a second control
  only while its defect lives (rmbp will announce the fix; leg does not depend on it).
- (18:05Z) pi 9's sweep item in my lane VERIFIED at 98213b7f: `UnaFS::format` call sdmmc_tegra.rs:1863 is inside
  `fn format_unafs_volume` (declared :1796, `#[cfg(feature = "install_target")]` at :1795), whose ONLY caller is
  :2100 inside `sdmmc_install_from_usb`, re-exported under `cfg(all(tegra, install_target))` at :117. Compiled out of
  the plain jetson image (esp_jetson forces tegra/tegrasmp/bsptick/bsprun and, after v4, sdmmc — never
  install_target). The hard alien-enemy shape on the Orin is unreachable by default; §11 stands. Pi: installer
  three-gated, module declaration gated (lib.rs:92); open on pi's queue — whether Gate 3 confirms at RUNTIME.
- (18:15Z) rmbp 16: R25 recorded (`docs/dev/RULINGS.md` on hw-rmbp `631138e1`). B91: INSTALL-SELF guards HOME only
  (never erase the boot device) and leaves every OTHER disk a candidate by construction — the inverse of R25; on the
  rMBP the other disk is Catalina's SSD, safe today by BLINDNESS (no AHCI driver) → the stranger guard must land
  BEFORE the AHCI driver. Audit queued there: unattended `install_probe_once` (main.rs:1800/:6031). REWORDING MY §11
  FLAG per rmbp: `sdmmc_install_from_usb` is "knob-gated, therefore REACHABLE by a build" — not "absent". General
  form (rmbp): the codebase guards what it OWNS and leaves what it merely SEES unguarded.
- (18:25Z) pi 9 CLOSED their item by reading install/pi.rs: Gate 3 is a SELF-CLONE of the seated card (home, not a
  stranger); runtime refusal on an empty tree before any write. Residual = bench hygiene: consent is build-time,
  the about-to-destroy line is a println not a prompt, "home" = whatever card is seated → a _CONFIRM image is a
  card-eater for any card seated in that board, and all three boards' cards meet at one host reader (how the Orin's
  card became a Pi card). Same axis applies to the Orin's install_target image.
- (18:30Z) rmbp 16 (B92/Q3c): the per-slot-target-dir matrix lever is arroyo's OWN documented pattern (:2062, :5045,
  :2452 name `--target-dir` per feature-set fingerprint) unapplied to KERNEL_CFG_MATRIX (:2683). QUEUED HERE as
  MATRIXPAR: cut on a branch from the bootroot+keep13 merge (three arcs touch the matrix block; sequencing avoids a
  three-way conflict), same legs same verdicts, RED-first: a leg made to fail must still fail under parallel slots;
  rmbp reads the diff same-turn. NOT Peter's B8 (that is the kernel8-test wall); this is pure build tooling.
- (18:40Z) rmbp 16 on MATRIXPAR: affinity risk. CHECKED AGAINST THE RUNNER (par_check.sh:2,25): (E) used leg i →
  slot i%P (index round-robin), and warm was 1.9 s with every slot holding 9 DIFFERENT feature sets → cargo retains
  per-fingerprint artifacts in one dir; (F)'s 8–12 s legs were legs landing on a slot that had NEVER seen their
  fingerprint (slot0-only run: its own 5 legs 0.1 s, the other 22 slow). So the load-bearing property is a STABLE
  leg→slot map, not feature affinity. MATRIXPAR brief conditions: (1) map keyed by LEG NAME (hash), not by index,
  so inserting a leg does not shift every other leg to a foreign slot; (2) a red names its leg; (3) the summary
  asserts legs-run == legs-listed so a vanished leg ≠ a passed leg; (4) RED-first: a forced-fail leg fails under P=6.
- (18:50Z) rmbp 16: the model (per-fingerprint coexistence) does not explain serial-warm 155 s. Verified from disk:
  serial-warm ran in the SAME default target dir right after serial-cold, nothing between → not other-verb eviction;
  mechanism of the 25× gap UNEXPLAINED. REPORT §6 addendum written; MATRIXPAR must re-measure cold/warm/warm-after-
  test on its own tree and promise only that. The isolation is right regardless (default dir is shared by every verb).
- (19:00Z) rmbp 16: is the serial-warm 155 s CPU or waiting? NOT on disk — (E)'s "11.5 s" is a per-leg WALL sum
  (0.2 s × 56), and no run recorded user/sys. MATRIXPAR brief gains: per-leg `/usr/bin/time -f '%e %U %S'` for every
  measured row, and the serial matrix run TWICE back to back in an isolated dir (run 2 fast ⇒ run 1 was never warm;
  both 155 s ⇒ real repeatable cost). Both decide whether the brief promises "stops rebuilding" or "stops waiting".
  Not run now: v4's gates are on the box, and load is what corrupted orin 21's numbers.
- (19:05Z) rmbp 16: per-leg arithmetic (parallel 0.2 s/leg vs serial-warm 2.8 s/leg, same op, same-warm dir) rules
  out overlap — the serial legs DID something the parallel legs did not. MATRIXPAR measures per-leg %e %U %S in
  both modes and compares per leg, not totals. Their last word on MATRIXPAR; they read the diff when it cuts.

## 13. (19:15Z) ⭐ BOOTROOT v4 DONE — `exec-orin22-bootroot` b0536d83 · b1885dbc · b12decbb (verified `git log`),
15 files +1078/−407, new `fs/bootdisk.rs`. GATES: check 0 ❌ (55 legs; baseline 56 — my "60" was matrix+4 base),
GATE-KNOB 0 phantom/0 dead, GATE-LEDGER 125 · kernel8-test 300 **125/125, floor unmoved, PI-GRANT not exercised** ·
test-arm 0 (only delta: FRGUARD ARMED line on aarch64) · esp-jetson plain builds, banner `…,sdmmc` · x86 elf
1907000→1903312 B. THE WITNESS (Pi, QEMU): `[vfs] root = boot volume serial=0xf3d9b41a source=global
match=/KERNEL8.IMG unafs=present matches=1 window_off=0x80000 window_len=4096 file_off=0x0 candidates=12 …`.
Mutations RED: m1 byte flip → NONE · m2 decoy → REFUSED multiple-kernels (FOLLOWUP changes this to first-found per
Peter) · m3 memory side → NONE and MBENCH 121/125 (the 125 is not vacuous). Window `_start`, .text by readelf, 0
relocs in RE. Opt-out UNAOS_NOSDMMC proven both directions by strings (controls `:: SDMMC:` / ORIN-SDMMC-1 /
TEGRA-SD — my `[sdmmc]` control was a wrong tag). STOP (accepted): test-arm cannot exercise the finder (no verb
headless; ESP on virt's default bus, no virtio-blk; xHCI one storage slot) — the Pi leg is the end-to-end proof.
Deviations (all sound): new_source reused; FatFs::read_at exists; `disks=global=`; unafs::MOUNT is handle-
DISCOVERING (UNAFSBIND unafs.rs:522) so `/` native iff discovered handle == matched disk; no `[sdmmc] root` spec
rows ever existed (knob UNFLOWN) → two PENDING rows `\bwindow_len=4096\b`, `\bmatches=1\b`; `/` not added to
RESERVED_VOLUME_PREFIXES (empty table already answers -ENODEV, shell.rs:417-424).
SPAWNED: FOLLOWUP (merge keep13 → home-soil rule/dedupe/bus mounts/stamp/comment repair, same branch) ·
MATRIXPAR (`exec-orin22-matrixpar` off b12decbb; RED-first; measure.sh, run only on a quiet box).
PUSHES: `git push origin exec-orin22-bootroot exec-orin22-keep13 exec-orin22-lawsnom2 exec-orin22-matrixpar`
- (19:25Z) pi 9 precision: 125/125 is hw-jetson's kernel8-test count at THIS tree; PI'S floor at pi's tip is 120
  (their parse_spec) — never quote another tree's number as theirs. UNAFSBIND handle-discovery is already in pi's
  tree (unafs.rs:520-522) — not fold-introduced. Grant #10 stays open, unspent, same five conditions if FOLLOWUP's
  bus mounts move a pi row.
- (19:35Z) rmbp 16 SECOND EYES on BOOTROOT: ACCEPTED, all three commits, read at the shas — cond.1 met (the false
  sentence quoted and dated), cond.3 met (0 hits both files), cond.2 exceeded (window proven by readelf, 0 relocs);
  `window_off` (runtime) and `file_off` (derived) both printed — keep both; R25: bootdisk.rs writes nothing;
  main.rs:2051 933-char fold verified (first `//` at col 194, four statements before it). Their queue: a mechanical
  "no `;` after the first `//`" gate (B76 class). ARC IS PEER-REVIEWED at b12decbb; FOLLOWUP's commits get the
  same read before landing.
- (19:45Z) PETER: "that label is a mistake. what if there are multiple bootable UnaOS disks? … why do you keep trying
  to hard code things like 2 disk?" — the label "two-card" is STRUCK everywhere (now "home-soil rule"). The code
  has no count: walk over all disks, `matches=N` + full list, indexed mounts for N. Other OSes are TOLD (UEFI device
  path → root=UUID / boot-uuid / BCD); UnaOS finds itself by content so the boot method is not load-bearing.
- (19:55Z) PETER: "what if the disk has a label? … /usb0 and /usb1 are meaningless outside the kernel." → mount
  points by VOLUME LABEL (`/volumes/<LABEL>`, serial fallback, suffix on collision — the macOS/Linux shape); no
  bus/slot/index in paths. Amendment 03 v3; applied after FOLLOWUP reports (LABELMOUNT, small).
- (20:10Z) PETER on labels: no scary serial in paths; detect unnamed; defend against hostile labels. → v4: FAT label
  is a fixed 11-byte field (copy exactly, no length trusted — overflow impossible by construction); unnamed = all
  spaces / `NO NAME` → `/volumes/Untitled`; whitelist sanitizer; altered bytes announced as `label_raw=` hex.

## 14. (20:20Z) PETER: "so many changes result in these huge tests — can we not bundle the tests so we don't spend
## most of the time waiting for each individual executor's build tests to complete."
RULE (applies from here; for LAWS §Gates at landing): per executor commit = TARGETED gate only (the touched arch's
`check`, the ONE QEMU leg that exercises the change, the commit's own RED-first mutation); the FULL battery runs
ONCE on the INTEGRATED tip, seat-run, as the arc's DONE gate; no gate for a change that cannot affect it (docs/spec
→ no kernel battery). FOLLOWUP/MATRIXPAR finish on their old briefs (no channel); LABELMOUNT and everything after
run under this rule. Levers already in flight: MATRIXPAR (matrix wall), B8 kernel8-test fast mode (Peter's decision).
- (20:30Z) pi 9 SHARPENS §14 (adopted): "cannot AFFECT the gate" ≠ "the gate cannot SEE it". Skip a gate only when
  the change cannot affect what the gate ASSERTS — and NAME the assertion; "no row for this" is a reason to ADD a
  row, never to skip. A skip with a named assertion is auditable; a bare "n/a" is not. (Today's instance: /usb
  posture — the pi spec is BLIND to /usb, not unaffected.) This is the LAWS wording.
- (20:40Z) rmbp 16 B94 (adopted into §14): "cannot affect it" must not be read as "it's only a comment". A comment
  inserted in a .rs file moves `panic::Location` line numbers and therefore image bytes (arroyo:6866, :341;
  byte-identity legs :6137/:1124/:3229). SAFE FORM: no build gate only for changes touching NO .rs file; any .rs
  edit — comments included — takes the targeted gate unless LINE-NEUTRAL, asserted by COUNTING lines, not intent.
  Instance in flight: FOLLOWUP's sdmmc_tegra.rs:37 comment repair (one line → one line; its old-brief full battery
  covers it this time). rmbp queues two mechanical checks: line-neutrality per commit; "no `;` after first `//`".
- (20:50Z) pi 9 CORRECTS the bounding rule (adopted, by their execution table): `\b` guards only against WORD-char
  extension (`2048` vs `20480`); against a sibling that differs by a NON-word char (`window` vs `window-band`) `\b`
  is INERT (`-` satisfies it) and `(?=\s|$)` is mandatory. Never a trailing space (parser strips → key inverts).
  LAWS wording: choose the bound by what the SIBLING TOKEN STARTS WITH, and a new bound ships with a NEGATIVE
  CONTROL that matches the sibling it must exclude — not merely a green run. BOOTROOT's two PENDING rows
  (`\bmatches=1\b` vs `matches=12`; `\bwindow_len=4096\b`) are word-char siblings → still sound; every later row
  (FOLLOWUP's `rw=`, `sha=`, LABELMOUNT's `/volumes/`) is checked against this rule at the seat's read.
- (20:55Z) pi 9 EXECUTED the two BOOTROOT rows both directions (sibling wire excluded exit 1 / real wire matches
  exit 0): both stand. Standard form for every new bound: TWO wires — the sibling it must exclude AND the real one
  (a bound that only excludes could exclude everything).
- (21:05Z) pi 9 + rmbp 16: the bounding rule lands as the FOUR-ROW EXECUTION TABLE, not prose — rmbp found the
  "use \b" prose had already spread into their B85 row and focus queue untested against a hyphenated sibling.
  "A rule of thumb travels faster than its test." Form for LAWS and every brief: the table with both wires
  (window-band: trailing space FALSE HIT, `\b` FALSE HIT, `(?=\s|$)` correct, `.*declines=` correct; 20480:
  `2048\b` correctly rejects). Provenance: trap rmbp, `\b` failure pi by execution, spread-detection rmbp.

## 15. (21:15Z) PETER: "yes" — B8 DECIDED: kernel8-test ordinary runs exit at the last COMPLETE marker + grace
(phase-unbound patterns live through the grace); the arc's DONE gate keeps the full 300 s wall. FASTK8 spawned
(`exec-orin22-fastk8` off b12decbb; arroyo/mbench only; gate = the k8 leg in both modes + m1 red-in-grace, m2
red-beyond-grace hidden-by-design demonstrated once, m3 no-marker fallback). FLEET: FOLLOWUP · MATRIXPAR · FASTK8.
PUSHES: `git push origin exec-orin22-bootroot exec-orin22-lawsnom2 exec-orin22-matrixpar exec-orin22-fastk8`
- (21:25Z) pi 9 on FASTK8: 20 s grace covers `[shellup] t=12908ms` (good); `[u7stk] hw=` is a monotonic high-water
  mark → a fast capture reports a FLOOR, true and wrong. ASK adopted: stamp the fast exit ON THE WIRE (trailer line
  in the captured log) so fast/full captures are distinguishable after the fact; accumulators measured on the full
  wall only. fastk8/BRIEF-AMENDMENT-01.md; seat follow-on if FASTK8 lands without it.

## 16. (21:40Z) PETER: "wtf is kernel8 why is there an exception for this" — kernel8.img is the Pi FIRMWARE's
filename for a 64-bit kernel; `kernel8-test` is the Pi QEMU verb. VERIFIED: `test`/`test-arm` blind-sleep the same
way (arroyo:2532 `sleep "$secs"`); the fix had followed the measurement (Pi) instead of the mechanism. FASTK8
STOPPED (it had one commit 26f6ee64, unreferenced, harvestable); QEMUFAST spawned on `exec-orin22-qemufast`: ONE
helper in the shared runner, default fast for every verb, `UNAOS_QEMU_FULL=1` for the DONE gate, completion signal
DERIVED per verb (only pi4-regression.spec declares COMPLETE today — 2; x86-witness 3, x86-wifival 1 exist but
`test` does not replay them via mbench; jetson-jd5/x86-fat/x86-wc/rmbp-boot declare none) — verbs with no signal
keep the wall and say so; trailer stamped in the log (pi 9). FLEET: FOLLOWUP · MATRIXPAR · QEMUFAST.
PUSHES: `git push origin exec-orin22-bootroot exec-orin22-lawsnom2 exec-orin22-matrixpar exec-orin22-qemufast`
- (21:55Z) pi 9: a trailer line joins EVERY spec's matcher population — a bare `COMPLETE` in it could satisfy a loose
  end-of-run marker and disable TRUNCATED detection silently. EXECUTED HERE over all 11 specs (mbench.Matcher.feed_raw,
  three trailer forms as briefed: "completion at…", "FULL wall=", "no completion signal"): 0 hits everywhere
  (jd5 19, sync1 134, pi4 266+9, rmbp-boot 23, round6 60, x86-fat 69, holocron 18, wc 11, wifival 31, witness 87).
  The brief's wording never says the bare word COMPLETE; keep it so. Re-run this check on the executor's final text.
- rmbp 16 B95: NO arroyo verb runs any x86 spec (only pi4-regression / pi4-barename are `--spec`'d; x86 verbs assert
  via DEFAULT_FORBIDS arroyo:2259) → x86 exposure of the COMPLETE change is theoretical; their ask = label a fast run
  on the wire (already in the brief). Their rule, adopted for my gate work: "a check being CORRECT and a check being
  RUN are different questions." QEMUFAST will find `test`/`test-arm` have no spec-declared completion → those keep
  the wall and say so; wiring an x86 spec into the verb is rmbp's (queued).

## 17. (22:05Z) ⭐ MATRIXPAR DONE — `e90c69ed` on exec-orin22-matrixpar (arroyo only, +146/−12; verified). P workers
(UNAOS_CHECK_P, default 6), slot = cksum(leg name) % P, per-slot dirs, output replayed in list order (lines/counts/rc
unchanged), heartbeats live. RED A: bogus feature → `❌ arm-tegra-tcurx` named, exit 1. RED B: dropped leg →
`legs-run 54 != legs-listed 55 — vanished leg(s): x86-mix-4`, exit 1. Green: 55 legs, rc 0. Own bug caught before
ship (`[ -s ] && cat` under set -e). MEASURED (load 1.2): **orin 21's 155 s serial-warm does not reproduce —
warm serial is 3.1 s; the 81× is WITHDRAWN**; the real win is COLD 387 → 152 s (2.54×). REPORT §7 addendum written.
CLAUDE.md knob list lacks UNAOS_CHECK_P (seat's, at landing). FLEET: FOLLOWUP · QEMUFAST.
- (22:15Z) pi 9 RETRACTS the wording remedy (a magic string standing in for a structural property — Peter's no-
  hardcode ruling applies): a diagnostic line is excluded from matching BY CONSTRUCTION. Resolution: the serial log
  stays pure guest bytes; the harness writes `<log>.run` sidecar metadata (mode/completion/grace/wall/cap) and
  mbench's verdict line prints the mode from it. qemufast/BRIEF-AMENDMENT-01.md; seat follow-on if QEMUFAST lands
  with an in-log trailer. The 11-spec check stays as a fact, not as a control anything depends on.
- (22:25Z) pi 9: the sidecar read is three-valued (fast/full/UNKNOWN incl. STALE); unknown must be sayable in the
  verdict; staleness detected by the sidecar carrying the log's identity (size+sha at write). Added to
  qemufast/BRIEF-AMENDMENT-01.md.
- (22:55Z) SEAT COMMIT `1dcd5644` on exec-orin22-matrixpar (+24/−10): UNAOS_CHECK_P unset = one worker, no
  --target-dir, default dir (the 3.1 s warm path, default); `=N` opt-in cold lever with price in banner; `=1`
  labelled slow-cold. Gate (bundled rule; load 1.9–2.4): `./arroyo check` rc 0 55 legs 16 s · `UNAOS_CHECK_P=6`
  rc 0 55 legs 12 s · leg lines IDENTICAL by diff to MATRIXPAR's gate-final. rmbp reads the diff. FLEET: FOLLOWUP · QEMUFAST.
- (23:05Z) FULL-OUTPUT DIFF of `./arroyo check` old path (BOOTROOT's check-final.log at b12decbb) vs the new default
  mode (1dcd5644): 453 diff lines, ALL classified, residual 0 — heartbeat 55 (introduced, the one permitted class),
  mode banner 1 (introduced), cargo "Blocking waiting for file lock" 4 (other cargos on the box), host unit-test
  ORDER 16 (pre-existing nondeterminism), cargo status/timing lines 182. No parser of check's stdout exists in
  scripts/ or tools/ (only a doc and a spec comment mention `arroyo check`). seat-gate/full-diff.txt(.residual).
- (23:15Z) rmbp 16 ACCEPTED 1dcd5644 (read the code: empty _slotbase has one guarded consumer; no `/slot0` path
  possible). Their note (default mode still creates target/check-slots/.results) → comment-only commit on the
  branch, bash -n only (bundled rule: no assertion affected; shell file, no Location lines).

## 18. (23:25Z) PETER, verbatim: "FOR SURE I DO NOT WANT TO WAIT FOR THE OTHER PLATFORM'S TESTS TO WAIT FOR THE MORE
## DEFINITIVE METAL BOOT. FROM NOW ON FOCUS PLATFORM ONLY UNTIL METAL RESULTS ARE IN SO THE OTHER ARCH'S TESTING IS
## DONE WHILE WE MOVE FORWARD TO THE NEXT ROUND"
RULE (for LAWS §Gates): the gate BEFORE a metal boot is the FOCUS PLATFORM's own (Orin: aarch64/tegra check legs +
`esp-jetson` build + the change's own RED-first). The other arches' QEMU batteries (kernel8-test, x86 test, WC test,
test-arm) run in the BACKGROUND while the card is written and flown; their results are a LANDING condition, never a
flight condition. Applied: FOLLOWUP finishes on its old brief; LABELMOUNT / sidecar / the integrated tip get the
focus gate, then render11 staging, with the cross-arch battery started in the background at the same moment.
- (23:35Z) pi 9 on §18 (adopted, verified in my tree — FLAKE-1 text present): background QEMU concurrency is now
  POLICY, and FLAKE-1's documented failure is a FALSE GREEN (silent no-capture from a QMP port check-then-bind race
  across worktrees). Two clauses for LAWS §Gates beside §18: (a) a landing condition requires a VERDICT (rc 0) —
  "finished" is not "has a verdict" (rc 3 truncation / rc 4 no-verdict = no evidence yet); (b) concurrent runs
  pin distinct `UNAOS_QMP_PORT`s as part of the policy (default 4463/4464 are never shared). Fast mode shortens the
  race window but the landing run is the full-wall one — the contended run is the one that must be reliable.
- (23:40Z) ⛔ CORRECTION TO §18 — PETER: "NO I SAID UNTIL METAL TEST RESULTS ARE IN." The other arches' tests do NOT
  run during the flight, background or otherwise. SEQUENCE: focus-platform gate → stage → card → metal boot → metal
  result IN → THEN the other arches' batteries start, concurrent with the NEXT round's work. My "background during
  the flight" wording was wrong; the peer relays are corrected this turn. pi's FLAKE-1 clauses still apply — to
  the post-metal concurrency (other-arch batteries beside the next round's executors), not to the flight.
- (23:50Z) rmbp 16 (R27 recorded there): `check` has no single-arch mode (one verb, check_both compiles both) and is
  3.1 s warm → the focus gate keeps the whole `check`; "focus platform only" reaches the QEMU batteries only. LAWS
  sentence. Their clause adopted: the landing report NAMES the post-metal battery result and sha, or the landing
  does not happen (a battery nobody reads = a check that does not run). Their point 1 ("flight proceeds while
  test-arm runs") predates the correction — under §18-corrected nothing runs during the flight.

## 19. PETER: "LOOK AT HOW REPEATEDLY I HAVE HAD TO HAVE YOU REWORK YOUR PRODUCT — WHY IMMEDIATELY
## START TESTING GARBAGE." The pattern, named: spawn on a first brief, the design keeps moving through the peer
round and Peter's rulings, the executor burns its hour on batteries for a dead design (BOOTROOT ×4, FASTK8→QEMUFAST,
MATRIXPAR default rework). RULE (LAWS §Arcs): THE DESIGN CLOSES FIRST — a brief is frozen only after the peer round
and Peter's word are in; no gate runs until it is frozen; the build executor works WITHOUT batteries; ONE gate runs
once at the end on the frozen design. Applied: FOLLOWUP STOPPED at step 3 (committed: merge 26f6ee64 + eb9996cc
home-soil rule w/ superseded bus-indexed mounts; uncommitted: stamp/dirty-suffix/comment repair) and QEMUFAST
STOPPED (uncommitted: arroyo/mbench/docs, in-log trailer form; found test/test-arm have no completion signal —
LEDGER S8 already names them negative-only). NEXT: ONE executor, INTEGRATE, harvests both worktrees, applies the
frozen amendments (label mounts v4, sidecar, stamp), commits, and runs ONLY the focus gate (check + esp-jetson +
strings census) → render11 staging. Finder mutations / kernel8 / x86 proofs run AFTER the metal result (§18).
- (00:10Z) Harvested: followup-step3-uncommitted.patch (301 lines, 3 files) · qemufast-uncommitted.patch (350 lines,
  4 files). INTEGRATE SPAWNED (Opus): frozen set on exec-orin22-bootroot — amend eb9996cc's title (TWOCARD struck →
  HOMESOIL), label mounts v4, stamp+suffix+comment repair, QEMU early exit + sidecar; NO QEMU; one gate (check +
  esp-jetson + strings) → render11 staging. Peers told; objections cost code time, not batteries.
FLEET: INTEGRATE only. PUSHES: `git push origin exec-orin22-bootroot exec-orin22-lawsnom2 exec-orin22-matrixpar`
(exec-orin22-qemufast is folded into bootroot and deleted; keep13 already on origin).
- (00:20Z) rmbp 16: R27 corrected in both places (gate → stage → card → metal → result IN → then other arches'
  batteries concurrent with the next round). Their sharpening for LAWS: the deferred battery completes after the
  seat has moved on, so the landing report NAMING its result+sha is the only thing that closes the B95 gap.
- (00:30Z) pi 9: RESERVED_VOLUME_PREFIXES is a static spelling list; `/volumes/<LABEL>` is runtime → an absent
  volume would fall through to native root's bare -ENOENT again (VFS-4's P44 incident). FIX: derive "volume not
  mounted" from the live mount set under the one static namespace root `/volumes`. integrate/BRIEF-AMENDMENT-01.md;
  seat follow-on before the single gate if INTEGRATE does not read it. Pi rows naming /usb or /volumes: ZERO —
  blindness, not safety (third time); pi's queue item stands.
- (00:40Z) rmbp 16 on label mounts (their indexed-/usb condition withdrawn): (1) dedupe — IS in the frozen set
  (eb9996cc dedupes by (num_blocks, BS_VolID)); INTEGRATE must apply it to the non-root mounts too; (2) collision
  suffix deterministic by BS_VolID order, never enumeration order; (3) sanitize-to-empty → Untitled, own arm;
  (4) `.`/`..` rejected explicitly. All into integrate/BRIEF-AMENDMENT-01.md.
- (00:50Z) rmbp 16 (right, and it is Peter's scenario): DiskId (num_blocks, BS_VolID) merges CLONES — a card and its
  image share both → one card hidden from /volumes. FIX in the amendment: dedupe only on PROVABLE same-device
  identity from the block registry (the aliasing is by construction there); otherwise mount both, `aliased=ambiguous`.
  Root stays first-found; two clones count as two disks. Landing-blocking until in.
- (01:00Z) PETER, verbatim, the REASON behind §18/R27 (goes into LAWS beside the rule): "THIS WILL HELP WHEN WE START
  SELF HOSTING WHERE WE WILL BE REBOOTING THE VERY MACHINE WE JUST WROTE THE CODE ON. THAT'S WHEN THE HARD FOCUS
  COMES INTO PLAY. THE CODE IS STILL GENERALIZED BUT THE MACHINE ITSELF IS COMPILING AND TESTING **FIRST** THEN THE
  WIDER AUDIENCE GETS THEIR RUN."

## 20. (01:15Z) PETER, verbatim, DIRECTION (for ROADMAP + LAWS provenance): "when we have self hosting with healing
## the machine will run itself until it runs right and we do not want all that great work hard coded as an
## appendage special case situation we want to grow UnaOS"
Reading: self-hosting with healing is the OS growing, not a bench appendage; every primitive it needs (find own
disk by content, honest self-verdict from the wire incl. mode/truncation, focus-first self-gate, stranger/friend
disk rules) lives in the kernel and arroyo, generic, board-free. Nothing of this round goes into a bench-only tool.
- (01:25Z) CITATION CORRECTION (rmbp 16): my "B6"/"B8" were the orin-22 BATON's §B items (B6 = bench write tooling
  unversioned / load-card10 in scratch; B8 = kernel8-test blind-sleep decision), NOT rmbp-ledger ids (their B6 =
  splash row, B8 = Broadcom NIC). RULE adopted for every cross-seat citation from here: an id carries its FILE —
  `orin-22 baton B6`, `rmbp-ledger B6`, `LEDGER SO22`, `orin-ledger A28`. Earlier bulletin lines using bare B6/B8
  mean the baton. rmbp B97 (their ledger): `card-watch.sh` exists twice — the in-repo copy is the dead macOS stub,
  the working one is in ~/unaos-bench/tools — R28's test: "can the machine run it on itself?" Both copies fail it.

## 21. (01:35Z) PETER, verbatim, DIRECTION: "when in healing mode the machine will need an alternate boot disk with
## the last known good bootable version to help when there's a hard lockup of the test kernel"
Reading (for ROADMAP, not this arc): the known-good disk is a second UnaOS disk = home soil (already visible, left
alone); healing adds the FALLBACK below the kernel — the firmware boots the test kernel, and if no clean-boot marker
lands within a bound, the next boot goes to the known-good disk, which finds itself by content and roots there.
UEFI: boot-order fallback + a boot counter the kernel clears on a clean boot; Pi: the firmware's tryboot/autoboot
mechanism. Generic by construction; one disk = no healing until a second is added. No board named anywhere.
- (01:45Z) rmbp 16 on `orin-22 baton B6` (FOR PETER, both positions now on record): status quo is already the
  failure (zero commits ever touched flash-pi4.sh / stage-x86.sh / load-card.sh; load-card in scratch/orin11).
  Their position, which I share: version the bench scripts in-repo as explicitly-labelled SCAFFOLDING (pre-self-
  hosting; cannot fork; self-heal never depends on them); the MACHINE'S OWN write path is the in-repo `install/`
  engine, operator-gated BY CONSTRUCTION; and the stranger guard (rmbp-ledger B91) is a PREREQUISITE for any in-repo
  write tool the machine can run on itself (R25: no write on the kernel's initiative). Guard first, capability
  second — the same order they found for AHCI. Peter's decision; not acted on this arc.
- (01:55Z) pi 9 on healing/A-B (roadmap): (1) two kernels on ONE disk — answered by the stamp, not by counting: the
  running image matches only the file carrying ITS build sha, so A and B (different builds) yield ONE match; only
  byte-identical clones collide, and those are the same version (first-found, Peter's rule; no REFUSE — pi quoted
  the pre-ruling text). Real gap: `mount_source` mounts ONE FAT partition per source → A/B as two FAT partitions on
  one disk needs per-PARTITION volumes (roadmap dependency, shared fs lane). (2) Pi image has no A/B slot (P1 FAT +
  P2 UnaFS 8 MB tail) → layout dependency. (3) install/pi.rs Gate 3 is whole-card by construction → healing and
  install collide in one function (pi's). All three on the roadmap item; none in this arc.

## 22. (02:05Z) PETER, verbatim, DIRECTION: "if there's no good bootable kernel a user will hopefully be able to
## reboot into linux, mac, or win to use our installer to assist in the initial healing process"
Reading: the HOST-SIDE INSTALLER is a PRODUCT (cross-platform Linux/macOS/Windows, in-repo, versioned) — the last rung
of healing when nothing on the machine boots. This reshapes `orin-22 baton B6`: the bench write scripts are not
scaffolding to preserve, they CONVERGE into the installer (one verb: put a known-good image on a medium, verify it,
with load-card10's named identity refusals). Existing pieces: `tools/unafs` host CLI, the image builders.
Healing ladder, all generic: (1) test kernel boots → self-verdict; (2) fails → firmware fallback to the known-good
disk (§21); (3) nothing boots → the user boots another OS and runs UnaOS's installer (§22).
- (02:15Z) pi 9 asks which finder is live: BOTH — the window-compare is the mechanism (a named .text range vs the
  file's bytes at the derived offset), and the build stamp LIVES INSIDE the compared bytes (item 5': a .text-placed
  stamp the window covers). So pi's BSS requirement stays load-bearing; nothing stale. Edge for the roadmap item:
  during a slot promotion (identical kernels, different volume contents) first-found picks a VOLUME arbitrarily —
  the promotion tool must make the slots distinguishable (a different build stamp per promoted copy, or the
  promotion marks the slot) before A/B ships.
- (02:20Z) rmbp 16 on the healing fallback (ROADMAP input, recorded beside Peter's §21 words which stay primary):
  (1) the loader uses NO UEFI variable services today (grep: none) — BootNext/BootOrder is a capability from zero;
  (2) rmbp-ledger A4/R3: Apple firmware selects via its own picker/bless, not standard boot variables — a variable-
  based fallback is unproven there and inapplicable on the Pi (no UEFI). THEIR SHAPE, generic below firmware: the
  boot medium carries BOTH test and known-good kernels; OUR loader chooses; the attempt counter is a FILE on the
  boot medium (loader writes before handoff, kernel clears on a clean boot) — the one capability all boards share
  is reading their own boot filesystem; composes with the content finder. The second UnaOS disk stays home soil,
  not the mechanism (B90). OPEN for the roadmap item: Peter said "alternate boot DISK" — a separate medium survives
  a corrupted boot medium; the loader-chooses shape survives firmware that ignores variables. Both may be needed;
  the Pi's chooser is the firmware's tryboot (no loader of ours runs before kernel8.img). Not this arc.
- (02:25Z) PETER: "remember we have a superpower system that can export perfectly native apps" → §22 sharpened: the
  host-side installer is a HANDLER exported as a native app per host OS through the existing export system
  (ROADMAP:258 — handlers export to platform apps; `vessels/una` on macOS today), not a script. The write-path
  identity refusals (load-card10's) live in that handler once; the bench scripts converge into it.
- (02:35Z) pi 9 SCOPES their own earlier answer (attached at their request): the DENYLIST policy for the card writer
  (refuse iff JETSON/RMBP, UNKNOWN warns) is BENCH-SCOPED — three known cards. The installer-as-product INVERTS it:
  on a user's machine every disk is a stranger, so refuse anything not POSITIVELY identified as target media; UNKNOWN
  refuses. load-card10.sh's refusal machinery = the product's safety design sitting in scratch; identify-card.sh =
  its "which disk am I about to write" step; positive evidence, never absence of a known-other. §22.
- (02:45Z) rmbp 16: scaffolding position withdrawn for Peter's product framing. Two scopings, both adopted: (a) B97's
  "can the machine run it on itself" is the test for SELF-HEAL primitives; rung 3's test is "can a user on a stock
  Linux/macOS/Windows box run it with no toolchain"; (b) MY "operator-driven by construction = safe" was BACKWARDS —
  on a user's machine everything is a stranger and the operator can be wrong, so the installer needs a STRONGER
  guard than the kernel's: the named identity refusals (load-card10's, the fleet's only prior art — stage-x86.sh
  stages and never writes) are the product's PRIMARY safety feature, positive identification only (pi's inversion).
  card-watch.sh is a watcher, not a seed of rung 3 (rmbp checked, dropped a name-based inference).
- (02:55Z) pi 9 PRECISION on §22 (verified here: vessels/ = aether-shell, facet, lumen, phonolite, pulse, una; no
  artifact named "export system"): the mechanism is the VESSEL — "an executable a user runs: Tokio runtime, the
  message bus, a selection of handlers, and a native GUI window" (vessels/facet/README.md). Product form of the
  installer, precise: an INSTALL HANDLER composed into a VESSEL built per host OS. Halves: `libs/fs/unafs` (volume
  logic, the kernel's frozen on-disk format) · `tools/unafs` (CLI = handler operations with an argv front end; the
  seed of the HANDLER) · the vessel (wiring/lifecycle/window — does not exist yet). The "which disk" positive-
  identification rule lives in the HANDLER: one implementation for every host OS, which is what removes the
  three-diverging-bench-writers problem. Peter's "export perfectly native apps" = build the vessel per host.
- (03:05Z) rmbp 16 for the roadmap item (§22): the vessel solves DISTRIBUTION, not PRIVILEGE — a raw-disk write needs
  root + unmount on macOS (`diskutil unmountDisk`, /dev/rdisk), admin + volume lock on Windows, root/udisks on
  Linux; nothing in vessels/handlers/libs/tools does a privileged host-disk write today (one README mention). Rung 3
  needs a privileged-helper story per host OS, designed up front, not bolted on. Their own correction: "our loader
  chooses" is itself an appendage on the Pi (no loader of ours before kernel8.img); the synthesis (loader-chooses on
  the medium + other-disk fallback where firmware can, separate disk survives a bad medium) stands. Lead for pi 9
  (unverified by either seat): Pi firmware's one-shot `tryboot` flag — already pi's queue item.

## ⚠ TIMESTAMP CORRECTION (real clock, `date -u`, at close): the "HH:MMZ" labels on entries from §5 onward were
## written as SEQUENCE labels by the seat and drifted past midnight; they are NOT clock readings. The whole round
## ran 2026-09-08 14:10Z → ~18:00Z. Order is right; the times are not. Everything dated "09-09" is 09-08.

## 23. CLOSE (real clock ~18:05Z 2026-09-08). INTEGRATE DONE: exec-orin22-bootroot tip 600887c2 (5 commits), focus
gate green, render11 candidate staged (kernel.elf 6dbeda7d…), NOT flown. Known gap: clone-vs-alias not in (grep 0).
Close report committed on hw-jetson (docs/dev/evidence/orin22/CLOSE-REPORT.md). Baton orin-23.md written (support
seat). Resume memory rewritten. Dead BOOTROOT worktree removed. orin 22 stops; Peter to the cafe; rmbp 17 + orin 23.
PUSHES: `git push origin hw-jetson exec-orin22-bootroot exec-orin22-lawsnom2 exec-orin22-matrixpar`
