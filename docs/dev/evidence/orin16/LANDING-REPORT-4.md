# DRAFT — orin 16 LANDING REPORT 4 (the orin 15 + orin 16 arc, 2026-09-06)

> **DRAFT.** Written by executor CLOSEDOCS at 2026-09-06T15:0xZ, **before** the render8 flight and **before**
> the merge. Nothing in this file has landed. The seat finalises it after render8 and the `--no-ff` merge:
> the tip below is moving (KEYDOORS-FIX folded during the writing), the peer-ack row is empty, and §7's
> battery table is a placeholder with no numbers in it. Do not cite this file as a landing record until the
> banner is gone.

Companion: [`CLOSE-REPORT.md`](CLOSE-REPORT.md) (the round, the flights, the owed list).

---

## 1. The arc

| fact | value |
|---|---|
| branch | `hw-jetson` |
| trunk | `main` = `f49ea1e7` (`Merge hw-jetson: orin 14 — R17 flown on the Orin…`) |
| merge-base | `671a0334` |
| arc tip at write time | `6aef5227` — **moving**; the arc was `8b696271` when this draft opened |
| arc size | **88** commits, `f49ea1e7..6aef5227` |
| `origin/hw-jetson` | `906e3aef` — **36 commits owed to origin** |
| sessions covered | orin 15 (render6's consequences, the post-kill folds) **+** orin 16 (the render7 flight, the four probes, the nine-executor round) |

Command of record for the range: `git log --oneline f49ea1e7..HEAD`.

---

## 2. The commits, grouped by subsystem

**Code and gates: 41. Docs, evidence and ledgers: 47.**

### aarch64 / Tegra lane — 19 (orin's own lane, no grant required)

`9d3ed9ef` ORINCLICK (A20 composes as the code stands) · `5936f239` TCURX (the console's RX is the TCU
mailbox, not UARTC) · `33dc7811` SDMMCWRITE (CMD24 + CMD17 read-back behind a knob) · `a45af90e` NET4A
(RX ring + 32 buffers below 4 GiB) · `a5a62ffb` GA10B rung 2 (BPMP power + clocks, one PMC_BOOT_0 read,
symmetric restore) · `6f56eff8` A19FIX (the shell gets its own window) · `fb5d0d8a` TCURX2 (read and consume
the TCU RX mailbox — A16 rung 2) · `4dc88314` DRAGDEAD (feed pointer motion to the grab from the jd2 drain) ·
`f36cb82f` QUARRY (open the file browser on the cascade route) · `40a11ca9` + `092c85fb` CLICKDEAD (surface
the re-arm accounting) · `80ed35a4` FOLDFIX (the display_tegra.rs tail union rebuilt 3-way) · `906e3aef`
NET-5 (ring RE-FETCH probe; the buffer-17 latch does not follow from its own witness) · `42ad642b` DESKFIX
(a close that stays closed; the pointer sprite measured; witnesses that stop inflating each other) ·
`1b0e33d8` RXMERGE (one owner for serial RX — exactly-once, in-order) · `45d02b4b` sdmmcroot (bind the card's
FAT as the VFS root) · `5fc5506a` GA10B rungs 3 + 3b (firmware-residue read pass; the ladder's first GA10B
MMIO writes) · `ff983f3f` BSPRUN (the EL0-EL1CORE refusal closes at the boot core) · `bbe777f5` DRAGREL-A64
(a focus switch cancels a live drag on this arch too).

### `video/` — 13 (rmbp's lane; every one under a recorded grant, §3)

`453e88b3` PANELFIX (V-1..V-4) · `adb3b1cd` MENUBAR (the bar hosts the focused window's menus; pulsewin
publishes instead of drawing) · `3329eec6` CONSOLEQUIET (QUIET-PANEL for the aarch64 routed console) ·
`c355fe34` CRYSTAL (Restart/Shut Down act; `action=` stops lying) · `bb513370` CRYSTAL (crystal group flush
right) · `064e0baa` (the Pi's boot-chain clause; comment-only) · `b768331a` MENUBAR2 (Esc reaches the shell
door; drop-down under its title; every window gets Quit) · `b19b2865` SO1 (a closed window's id cleared from
every cache before the slot is re-issued) · `cd533543` PRTSCR-VOL (taken from hw-rmbp byte-for-byte) ·
`d7eec583` PRTSCR-ASYNC J1 (bounded slices on the VOL ladder; `Refusal::Vanished`) · `5f8f392f` (the
`usb_backed()` invariant named; comment-only) · `049d2a48` WINID2 (`SEAM_WIN` is the sixth holder) ·
`accca478` QUARRYDOOR fixture.

### `main.rs` — 6 (shared; granted)

`fc91eef9` PRTSCR-ORIN cadence · `99c153ca` the Pi's `render_service` drains `SHELL_REOPEN` too ·
`0f6a12d2` KEYDOORS F0 (both calls now precede all prose — the TABKEY restore) · `d77d41ed` F1 (Quarry's
whole keyboard reaches the SHELL doors) · `b8aa3d2c` F2 (the supstate shell door + the leg that compiles it) ·
`8bc79ddd` F3 (Esc reaches the Pi's `SCREEN_APP_ACTIVE` peek branch).

### `drivers/xhci` — 1

`7b143041` CLICKDEAD (count the silent pointer-completion exit; split dup vs no-buffer).

### Gates, specs and the builder — 2

`72555685` (`jetson-sync1.spec` FORBIDs a full winid registry) · `6abd97d2` (GATE-KNOB's trailing-comment
check regains its end-of-line anchor, plus a control probe).

### Docs, evidence and ledgers — 47

The orin 15 CLOSE-REPORT chain, the render6 flight results and excerpts, the S7 step-1 re-cut and its
identity proofs, `SERIAL-WATCH.md`, `GA10B-HISTORY.md` / `GA10B-LADDER.md` / `GA10B-RUNG3.md`, `NET4A.md`,
`NET5.md`, `RXMERGE.md`, `SDMMC-T1.md`, `MBENCH-FLAKY.md`, `A19-render7.md`, the render7 flight result with
its excerpt, scores, marks, harvest checksums and three reduced screenshots, the four probe excerpts and
scores, `PROBES-2026-09-06.md`, `FIXTURE_FLAKES` Class 3, and the running ledger ticks in
`docs/dev/OS/orin-ledger.md`, `docs/dev/LEDGER.md` and `docs/dev/RULINGS.md`.

---

## 3. Grant table

Lane rule: rmbp owns shared kernel-core and `video/`; pi owns the Pi legs; orin owns `arch/aarch64` +
`tegra`-feature files. Every out-of-lane file below was negotiated over ccd before it was touched, and the
grant is recorded in both sessions. **Times are as they were exchanged; the seat's own stamps between
~13:1xZ and 17:0xZ ran about three hours fast (real 13:37Z at the `gates-42ad642b` restart).**

| file family | grant | peer + message | conditions carried into the fold |
|---|---|---|---|
| `video/winmenu.rs` (PANELFIX) | — | **rmbp 12**, 2026-09-06 13:2xZ | applied as measured; `wm.rs` untouched |
| `video/menubar.rs`, `video/pulsewin.rs`, `video/strip.rs`, `video/crystal.rs`, `main.rs` (MENUBAR `adb3b1cd`) | orin 15 ask | **rmbp 12**; pi 7 asked for the C ack | ten files, 1403+/266−; x86 stated UNPROVEN in the row; patch file count is 10, not 11 |
| `video/fbcon.rs` (CONSOLEQUIET) | orin 15 ask | **rmbp 12** | x86 image byte-identical; the Pi panic-on-glass leg demonstrated |
| `drivers/xhci/mod.rs` (CLICKDEAD) | orin 15 ask | **rmbp 12** grant + **pi 7** re-accept | split the dup-drop counter (dup vs no-buffer); baseline chains are per tree — the ack is on the move and its reason, never on a sha value |
| `main.rs` S7 step 1 | orin 15 ask | **rmbp 12** conditions (a)–(e); **pi 7** ack | x86 `UNAOS_WC=1` byte identity by two-build diff; GATE-FAMILY stays 3; every mid-file hunk N→N, growth only at the tail |
| `arch/aarch64/sched.rs` (BSPRUN) | **B18** | **rmbp 13** | ordering-only (`mark_online` is an idempotent bounds-checked store); P7 trap checked, not assumed; `git apply --check` rc 0 |
| `video/pulsewin.rs`, `video/dock.rs` (DESKFIX A30) | **#2 / B19** | **rmbp 13**, 14:3xZ | A30 granted; **`pal.rs` hunk REFUSED as written** — callers enumerated, dropping the +1 shrinks x86's cursor and undoes the midden-trails fix; reshaped to converge UPWARD (SO5, owner rmbp) |
| `video/prtscr.rs` (PRTSCR-ASYNC `a86e3268`) | **#3 / B20** | **rmbp 13**, 16:0xZ | **BLOCKING**: `Job::begin` used `mount_program_source`, the call PRTSCR-VOL replaced → refuses forever on the rMBP. Executor **withdrawn** as a fold candidate |
| `video/prtscr.rs` (PRTSCR3) | **#3b** | **rmbp 13**, 14:2xZ; **pi 7** 14:3xZ | take the CONTENT DELTA, never a cherry-pick, and gate by file sha256 identity; pi's same-line `usb_backed()` invariant comment disclosed as one line beyond the granted hunks; `strings` splits the refusal line at its em-dash — use `grep -a` |
| `src/power.rs`, `video/crystal.rs` (CRYSTAL) | **#4a** | **rmbp 13**, 14:1xZ; **pi 7** ack 16:4xZ | body path is `src/power.rs` (arch-neutral, NOT `arch/aarch64`); cfg prose exact `all(target_arch="aarch64", not(feature="pi"))`; pi's two edits — the `not(pi)` comment reads "no EL3 PSCI monitor in the Pi's CURRENT boot chain" (a boot-chain consequence), and the leg-4 behaviour change sits ABOVE the `action=stub` labelling |
| `video/menubar.rs` (CRYSTAL flush right, SO4) | **#4b** | **rmbp 13**, 14:1xZ | own commit from the patch; zero `cfg(` → arch-neutral, moves x86 bytes; rows A33/SO4 say UNFLOWN on rMBP; rmbp flies at 1920×1200 before J1 |
| `video/winmenu.rs`, `menubar.rs`, `crystal.rs`, `main.rs` (MENUBAR2) | **#5** | **rmbp 13**, 14:2xZ; **pi 7** ack 13:5xZ | `wm.rs` NOT touched (4 files); `main.rs` 8949/8949; A10 row names the CLASS and the two surfaced defects; verify the `crystal::selftest` chain order at the folded tip; pi's row condition — desktop_firmware-armed Pi images only, knob-off byte-identical and correctly so |
| `video/wm.rs`, `video/dock.rs`, `main.rs` (WINID) | **#6** | **rmbp 13**, 14:1xZ (both halves + the Pi-drain `main.rs` extension); **pi 7** (b) 14:2xZ | all nine hunks verified N→N or tail; register the sixth holder or name it unregistered with a reason; the row states what a second `orin_wm1` call after a close does; pi's S4/S7 clause relayed |
| `video/wcg.rs` (WINID2) | **#6 addendum** | **rmbp 13**, 14:3xZ; condition closed 14:5xZ | sited on `wcg::begin`, **not** at the store line (`seam_glyph_note` runs in print context under a no-lock/no-print contract) — rmbp amended **B23**; gate `all(witness, any(x86 wc, aarch64 desktop_firmware))`; knob-off `kernel8` identity is **VACUOUS** (pi 7: `pub mod wcg;` is `cfg(witness)`, absent from the knob-off image) |
| `fs/vfs.rs`, `video/shell.rs` (ROOTFS) | **#7** | **rmbp 13**, 14:2xZ | add `target_arch = "aarch64"` to the `shell.rs` same-line cfg, statement BEFORE the line's first `//`; `sdmmcroot` not Pi-eligible (SR1 class) |
| `main.rs`, `arch/x86_64/syscall.rs`, `video/quarry/live.rs`, `unaos/scripts` (KEYDOORS-FIX) | **#8** | **rmbp 13** — asked, itemised (a)–(d); Pi behaviours → **pi 7** | fold only the F0/F1/F2/F3 `main.rs` hunks under #8; the aarch64 `drag_cancel("focus-key")` twin goes to rmbp as a patch, theirs to cut or accept; at fold the A10 row takes the F0 note |

**Lane exposure the announce must carry** (TRUNKPREVIEW §2, unblocking): the arc touches
`arch/x86_64/syscall.rs`, `drivers/xhci/mod.rs`, `main.rs` and thirteen `video/` files. The xhci grant is
already in its commit body; the peer-ack step confirms the grant record for the `video/` family and
`arch/x86_64/syscall.rs`.

---

## 4. TRUNKPREVIEW at `8d9627f5` (throwaway; nothing landed)

Full report: `~/unaos-bench/scratch/orin16/trunkpreview/PREVIEW.md`. Run on branch `preview-landing` in an
isolated worktree; no push, no stash, `main` never checked out or written. Preview merge commit `13db7477`.

> ## Verdict: LANDABLE

| check | result |
|---|---|
| conflicts | **0** — `git merge --no-ff --no-commit 8d9627f5` → "Automatic merge went well"; `--diff-filter=U` 0 files, `ls-files -u` 0 entries. **Nothing was hand-resolved.** No docs/ledger union case arose |
| conflict markers in tracked source | **0** real. Two files match the marker strings and both are benign pre-existing content, verified by reading every hit: `docs/dev/evidence/orin15/S7-STEP1-v2-resolve.py` (marker literals it asserts on) and `docs/dev/LEDGER.md:66` (hazard row P8 quoting the guard) |
| merge shape, half 1 — two parents | `git log --pretty=%p -1 13db7477` → `f49ea1e7 8d9627f5`. **PASS** |
| merge shape, half 2a — diff vs arc parent | `git diff 8d9627f5 13db7477 \| wc -l` → **0** |
| merge shape, half 2b — trunk original work since base | `git log --no-merges --oneline 671a0334..f49ea1e7 \| wc -l` → **0** (all three trunk commits are `Merge hw-jetson:`). **The zero diff at 2a is SAFE because and only because 2b is empty** |
| `./arroyo check` | **0** — 0 red; 49 cfg-gated kernel legs ✅; GATE-KNOB OK (159 features, 158 named by a cfg, 0 phantom, 0 dead); GATE-LEDGER OK |
| `./arroyo test` (x86 headless) | **0** — 51 PASS/witness-OK, 0 FAIL |
| `./arroyo test-arm 60` | **0** — 2 PASS/witness-OK, 0 FAIL |
| `./arroyo kernel8` | **0** — `kernel8.img` sha256 `8ff7c1d1f4e8938d…`, 1 254 984 B (the leg's own summary line prints the 64 MB flashable image's sha, not this one) |
| `./arroyo kernel8-test` | **0** — **MBENCH PASS 119/119 required witnesses, 0 forbidden hits, 9887 lines scanned**; 136 FORBID checks armed; 1 m 17 s, single run |
| `unaos/scripts/ledger-check.sh` | **0** — 102 rows in 2 ledger files + RULINGS |
| widened cross-ref resolver (rmbp 13's, over the merged ledgers) | **LATENT-RED = 0** — 90 distinct ids pooled across `LEDGER.md`, `orin-ledger.md`, `RULINGS.md`; 18 cross-refs matched, 0 unresolvable. Strictly stronger than the shipped gate |
| floor | 117 REQUIRE + 2 COUNT = **119 met exactly**; 133 FORBID + 3 default = **136 armed, 0 hits** |

**This preview does not transfer.** It ran at `8d9627f5`; the arc is far past it and still moving. The final
battery **re-runs on the final tip** (§7), and the preview's `ls-remote` freshness does not carry to the
merge (§6).

Two observations carried, neither a blocker: QEMU-green is not hardware-green (`kernel8-test` is the Pi QEMU
battery at 640×480 defaults; the Orin evidence is render8); and trunk currently holds only one arch ledger —
`docs/dev/OS/orin-ledger.md` — since `ledger-check.sh` SKIPs a missing ledger with a line, so trunk's ledger
surface is orin-only plus the shared files until pi and rmbp land theirs.

---

## 5. Merge-shape rule (LAWS §Code and history, pi 6 at LANDING-2 `d11cd56e`)

After the `--no-ff` landing, prove two facts with commands, **in this order**, and quote both here:

1. **Two parents.** `git log --pretty=%p -1 <merge>` prints the trunk tip **and** the arc tip. (A `checkout -b`
   during a conflicted merge once dropped `MERGE_HEAD` and left trunk on a single-parent commit; the next
   sync re-conflicted 386 commits.)
2. **A zero diff is safe only with its second command.** `git diff <arc-tip> <merge> | wc -l` = 0 is SAFE
   **if and only if** `git log --no-merges --oneline <merge-base>..<trunk-tip>` is EMPTY. Without the second
   command, a zero diff against a non-ancestor parent is indistinguishable from wholesale loss of trunk-only
   content.

| # | fact | command | result |
|---|---|---|---|
| 1 | two parents | `git log --pretty=%p -1 <merge>` | *(pending)* |
| 2a | diff vs arc parent | `git diff <arc-tip> <merge> \| wc -l` | *(pending)* |
| 2b | trunk original work since base | `git log --no-merges --oneline 671a0334..<trunk-tip> \| wc -l` | *(pending)* |

---

## 6. Landing-race rule

There is no integrator seat. The landing track runs the independent adversarial review itself (the COI guard:
the author seat never reviews alone), **announces the merge over ccd and obtains a peer ack from at least one
other track seat**, then merges with `--no-ff` and runs the trunk battery.

- **Every announce, ack, and repeat of an ask carries a fresh `git ls-remote` run that same turn, by both
  seats.** Reachability is never relayed stale. In-sandbox the plain form dies on publickey — use
  `flatpak-spawn --host`.
- **Immediately before the `--no-ff` merge**: announce over ccd and run `ls-remote` again. **If trunk moved
  since the review, merge the new trunk into the arc and re-run the trunk battery before landing.** First
  announced merge wins; the other seat rebases its landing on the result.
- **Silence is never consent.** No ack, or an objection the seats cannot resolve over ccd → the merge does
  not happen and the disagreement goes to Peter with both positions.
- Before announcing any sha to a peer, verify it with `git ls-remote --heads origin` **and**
  `git log --oneline -1 <sha>`. A sha the peer cannot fetch is not a deliverable.
- **The seat never runs `git push`.** Peter does. Push line: `git push origin hw-jetson` first, then
  `git push origin main` — arc before trunk, so nothing on trunk points at an unreachable commit.

| step | state |
|---|---|
| adversarial panel on the final tip | *(pending)* |
| `ls-remote` at the announce | *(pending — value + UTC stamp)* |
| peer ack #1 (rmbp 13) | *(pending — their `ls-remote`, their scope)* |
| peer ack #2 (pi 7) | *(pending — their `ls-remote`, their scope)* |
| `ls-remote` immediately before the merge | *(pending)* |
| merge commit | *(pending)* |

---

## 7. Trunk battery on the final tip — PLACEHOLDER

Runs in `../UnaOS` on the merge commit, **not** on the preview and **not** on the arc tip. No number below is
filled in.

| leg | exit | evidence |
|---|---|---|
| `./arroyo check` (both arches) | *(pending)* | red count; GATE-KNOB; GATE-LEDGER |
| `UNAOS_WC=1 ./arroyo test 150` | *(pending)* | banner must show `wc`; PASS count; panic count; `[ptrdead]` count |
| `./arroyo test-arm 60` | *(pending)* | line count; witnesses; panics |
| `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` | *(pending)* | `MBENCH … N/N required witnesses, 0 forbidden hit(s)`; **host load at the run** |
| `unaos/scripts/ledger-check.sh` | *(pending)* | row count; gate on the **exit code**, not the printed line |
| `kernel8.img` sha256 | *(pending)* | taken with `sha256sum`, not read off the leg's summary line |

**Three conditions on this table before it can be quoted.**

1. **The `kernel8-test` denominator moves to 120 when pi lands** — `hw-pi4`'s `f25f1601` spec has REQUIRE=118
   (the CHROMEBAND leg, `[wc-b] rollup … amp=1.00x`, toothless on Pi but it must be EMITTED). Checked on our
   own pidesk capture: 53 occurrences, so 120/120 will hold.
2. **SO7 is open and this leg is its subject.** `kernel8-test` reds intermittently under host build load with
   content-bearing hits, at loads as ordinary as ~3.9. **A quiet-box run is OWED before this battery is used
   as a gate.** A single red does not convict (pi's rule) — but a re-run habit is not the answer either: if
   the baseline rate is non-zero on a quiet box, the fixture becomes load-independent or the flaky leg gets an
   explicit allowlist (rmbp 13, B26).
3. **Gate runs longer than ten minutes die under the Bash tool cap with no result and exit 1.** That is not a
   red leg. Run the chain under `nohup` and poll a RESULT file.

---

## 8. Docs reconciliation owed at the landing (F-1)

- `docs/dev/OS/08_VIDEO/screenshot.md` §10/§11 were **written on `hw-rmbp`** (one-sided +70 at J1). **Do not
  write them here** — reconcile at the merge.
- The `hw-rmbp` copy of `docs/dev/LEDGER.md` does not yet carry `S28`–`S32`; that union is the landing seat's,
  by the standing rule (keep both tracks' additions).
- `SO6` is intentionally absent from `LEDGER.md`: the `arroyo`-from-repo-root false red is rmbp's **B22** and
  is cross-referenced, not re-filed.
- Cross-tree cross-refs (P14): an `→ X` in an id cell means the gate must resolve it. rmbp's resolver split
  (`44c7887b`) makes seat-prefixed ids (SR/SO/SP) DEFERRED — printed and counted — and **STRICT at the
  landing**. Every deferred ref must resolve once the ledgers merge.
