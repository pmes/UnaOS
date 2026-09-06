# orin 16 LANDING REPORT 4 (the orin 15 + orin 16 arc, 2026-09-06)

> **FINAL — LANDED.** `main c7407753` = `--no-ff` merge of `hw-jetson c5048fe6` into `f49ea1e7`, made in `../UnaOS` at 2026-09-06T16:42Z by orin 17 and on origin at 16:50Z (fresh host `ls-remote`: `main c7407753`). Trunk battery on the merge commit: green, every leg by exit code (§8). Peer acks: pi 7 16:40Z, rmbp 14 ~17:2xZ (§7). Draft history: opened by executor CLOSEDOCS 15:0xZ, §3 reconciled by GRANTREC/LANDDOC, folded by orin 17.

Companion: [`CLOSE-REPORT.md`](CLOSE-REPORT.md) (the round, the flights, the owed list).

---

## 1. The arc

| fact | value |
|---|---|
| branch | `hw-jetson` |
| trunk | `main` = `f49ea1e7` (`Merge hw-jetson: orin 14 — R17 flown on the Orin…`) |
| merge-base | `671a0334` (`git merge-base f49ea1e7 963516c8`) |
| arc tip | **`963516c8`** — `docs/evidence: orin 16 CLOSE — render8 … staged and on the card` |
| arc size | **91** commits, `f49ea1e7..963516c8` (`git rev-list --count`) |
| `origin/hw-jetson` | `963516c8` as of TRUNKPREVIEW2's `ls-remote` — **the arc tip was fetchable by a peer at that run**. That value does not carry: the announce and the merge each run their own `ls-remote` in the same turn (§7) |
| sessions covered | orin 15 (render6's consequences, the post-kill folds) **+** orin 16 (the render7 flight, the four probes, the nine-executor round) |

Command of record for the range: `git log --oneline f49ea1e7..963516c8`.

The three commits between the CLOSEDOCS draft tip `6aef5227` and `963516c8` (`1686268e`, `c24d9517`,
`963516c8`) are all documentation — the KEYDOORS-FIX ledger tick and the two close/landing drafts. No
code moved after `6aef5227`, so §2's code groupings stand at the final tip.

---

## 2. The commits, grouped by subsystem

**Code and gates: 41. Docs, evidence and ledgers: 50.**

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

### `main.rs` — 6 whose primary subject is `main.rs`; **17 commits touch the file**

`fc91eef9` PRTSCR-ORIN cadence · `99c153ca` the Pi's `render_service` drains `SHELL_REOPEN` too ·
`0f6a12d2` KEYDOORS F0 (both calls now precede all prose — the TABKEY restore) · `d77d41ed` F1 (Quarry's
whole keyboard reaches the SHELL doors) · `b8aa3d2c` F2 (the supstate shell door + the leg that compiles it) ·
`8bc79ddd` F3 (Esc reaches the Pi's `SCREEN_APP_ACTIVE` peek branch).

**The heading groups by primary subsystem, and that is not a file claim.** Seventeen commits in the
range touch `unaos/crates/kernel/src/main.rs` —
`git log --oneline f49ea1e7..963516c8 -- unaos/crates/kernel/src/main.rs | wc -l` → **17**:
`8bc79ddd b8aa3d2c d77d41ed 0f6a12d2 fc91eef9 99c153ca b19b2865 5fc5506a 45d02b4b b768331a 42ad642b
f36cb82f 4dc88314 6f56eff8 a5a62ffb 5936f239 33dc7811`. Eleven of the seventeen have every `main.rs`
hunk inside the orin-only `#[cfg]` region and rest on rmbp 12's tegra-region lane ruling (§3.2); seven
of those eleven touch `main.rs` and nothing else. The six listed above are the ones whose *subject* is
`main.rs`; the other eleven appear under their own subsystem headings. (Correction applied at the
landing — the CLOSEDOCS draft read "`main.rs` — 6 (shared; granted)", which reads as a file count.)

### `drivers/xhci` — 1

`7b143041` CLICKDEAD (count the silent pointer-completion exit; split dup vs no-buffer).

### Gates, specs and the builder — 2

`72555685` (`jetson-sync1.spec` FORBIDs a full winid registry) · `6abd97d2` (GATE-KNOB's trailing-comment
check regains its end-of-line anchor, plus a control probe).

### Docs, evidence and ledgers — 50

The orin 15 CLOSE-REPORT chain, the render6 flight results and excerpts, the S7 step-1 re-cut and its
identity proofs, `SERIAL-WATCH.md`, `GA10B-HISTORY.md` / `GA10B-LADDER.md` / `GA10B-RUNG3.md`, `NET4A.md`,
`NET5.md`, `RXMERGE.md`, `SDMMC-T1.md`, `MBENCH-FLAKY.md`, `A19-render7.md`, the render7 flight result with
its excerpt, scores, marks, harvest checksums and three reduced screenshots, the four probe excerpts and
scores, `PROBES-2026-09-06.md`, `FIXTURE_FLAKES` Class 3, the KEYDOORS-FIX ledger tick (A10/A27/SO8/SO9),
the orin 16 CLOSE-REPORT and this report, and the running ledger ticks in `docs/dev/OS/orin-ledger.md`,
`docs/dev/LEDGER.md` and `docs/dev/RULINGS.md`.

---

## 3. Grants — the F-1 record, per commit

Source: executor GRANTREC's reconciliation, `~/unaos-bench/scratch/orin17/GRANTS-RECONCILIATION.md`,
which resolved the twenty commits TRUNKPREVIEW2 could not clear from the commit bodies alone (§5 of
`~/unaos-bench/scratch/orin16/trunkpreview2/PREVIEW-2.md`).

`CLAUDE.md`'s lane rule: rmbp owns shared kernel-core and `video/`; pi owns the Pi legs; orin owns
GIC/timer, `arch/aarch64` and `tegra`-feature files. Every commit in `f49ea1e7..963516c8` that touches a
file outside orin's lane is accounted for here. **An ask is not a grant, and silence is never consent** —
the rows that are not fully covered say so in their own words.

**28 commits** touch `main.rs`, `video/**` or `arch/x86_64/syscall.rs`. Twelve state their grant in the
commit body; the other sixteen are resolved below against the round ledger
(`~/unaos-bench/scratch/orin16/ROUND-QUEUE.md`), the executor `PROGRESS.md` files and the tracked
ledgers. **Provenance caveat that applies to every row: all of these records are orin-side.** None is
the peer's own text — which is exactly why the announce asks for an ack. Times are as exchanged; the
seat's stamps between ~13:1xZ and 17:0xZ ran about three hours fast (real 13:37Z at the
`gates-42ad642b` restart).

### 3.1 Grants received, by ask number

| ask | peer + time | files granted | commits in this landing |
|---|---|---|---|
| PANELFIX | **rmbp 12**, 13:2xZ | `video/winmenu.rs`, `video/menubar.rs` | `453e88b3` |
| MENUBAR (orin 15) | **rmbp 12** | `video/menubar.rs`, `pulsewin.rs`, `crystal.rs`, `strip.rs`, `winmenu.rs`, `wm.rs` (scoped, condition J), `mod.rs`/`screen.rs` under standing grants, `arch/x86_64/syscall.rs` | `adb3b1cd` |
| CONSOLEQUIET | **rmbp 12** (standing parity grant; K–N verified in their tree) | `video/fbcon.rs` | `3329eec6` |
| CLICKDEAD | **rmbp 12** grant + **pi 7** re-accept | `drivers/xhci/mod.rs` | `7b143041` |
| **#2 / B19** | **rmbp 13**, 14:3xZ | `video/pulsewin.rs`, `video/dock.rs` — **`pal.rs` REFUSED as written**, reshaped upward, owner rmbp | `42ad642b` (carries no `pal.rs`; the refusal is honoured) |
| **#3b** | **rmbp 13**, 14:2xZ; **pi 7** 14:3xZ | `video/prtscr.rs` | `cd533543`, `d7eec583`, `fc91eef9`, `5f8f392f` |
| **#4a / #4b** | **rmbp 13**, 14:1xZ; **pi 7** ack 16:4xZ | `src/power.rs`, `video/crystal.rs`, `video/menubar.rs` | `c355fe34`, `bb513370`, `064e0baa` |
| **#5** | **rmbp 13**, 14:2xZ; **pi 7** ack 13:5xZ | `main.rs`, `video/crystal.rs`, `menubar.rs`, `winmenu.rs` — `wm.rs` NOT touched | `b768331a` |
| **#6** (+ addendum 14:3xZ) | **rmbp 13**, 14:1xZ; **pi 7** (b) 14:2xZ | `video/wm.rs`, `fbcon.rs`, `pulsewin.rs`, `quarry/live.rs`, `instgui.rs` (WINID-video.patch, 9 hunks / 5 files) + the Pi-drain `main.rs` extension; addendum: `video/wcg.rs` | `b19b2865`, `99c153ca`, `049d2a48` |
| **#7** | **rmbp 13**, 14:2xZ | `fs/vfs.rs`, `video/shell.rs` (+ the `main.rs` fold) | `45d02b4b` |
| **#8** (a–d) | **rmbp 13 — GRANTED, all four**; **pi 7** acked the Pi behaviours | `main.rs` (F0/F1/F2/F3 hunks), `arch/x86_64/syscall.rs`, `video/quarry/live.rs`, `unaos/scripts/knob-hygiene.sh` | `0f6a12d2`, `d77d41ed`, `b8aa3d2c`, `8bc79ddd`, `accca478` (quarry half), `6abd97d2` |
| **B18** | **rmbp 13** | `arch/aarch64/sched.rs` | `ff983f3f` |

**Correction applied at the landing, and it is the most load-bearing one here.** The CLOSEDOCS draft
recorded ask **#8** as *"asked, itemised (a)–(d)"*; so did `CLOSE-REPORT.md` §3 row 17 and its §4 table.
Both were written **before the grant arrived** and are stale. `ROUND-QUEUE.md`'s LANDED entry reads
*"KEYDOORS-FIX FOLDED (rmbp #8 a/b/c/d all granted, pi acked)"*, and the tracked ledger says the same in
git — `docs/dev/OS/orin-ledger.md` **A10**: *"KEYDOORS-FIX folded (orin 16, rmbp #8, pi ack)"*. Five
commits rest on it (`0f6a12d2`, `d77d41ed`, `b8aa3d2c`, `8bc79ddd` and `accca478`'s quarry half). Both
reports are corrected in this landing.

**Second correction: ask #6's file family.** The draft read *"`video/wm.rs`, `video/dock.rs`, `main.rs`,
`wcg.rs`"*. `video/dock.rs` is **not** in WINID-video.patch — it is #2/B19's file. Verified against
`git show --stat b19b2865`, whose video files are `fbcon.rs`, `instgui.rs`, `pulsewin.rs`,
`quarry/live.rs` and `wm.rs` — exactly the five files `winid/PROGRESS.md` records as the patch's 9 hunks
/ 5 files, all verified N→N or tail.

### 3.2 `main.rs`'s orin-only `#[cfg]` region — rmbp 12's lane ruling, and the commits that rest on it

rmbp 12 ruled (2026-09-06 03:0xZ, `~/unaos-bench/scratch/orin15/FOLD-NOTES.md:8`): *"`jd2_console_pump`
(main.rs:2807 cfg tegra) is orin's lane — no grant for the drag_route_tail call; MUST be a LINE-NEUTRAL
fold (P7, python position check) or every Location below :2808 renumbers and pi's/x86's baselines move
with every gate green."* pi 7 set the fold pattern and the proof the same night: cfg on the statement,
prose after the line's first `//`, and `sha256(kernel8.img)` identical before and after — or pi
re-accepts with the reason. The ruling is also **in git**, in the tracked ledger's owner column:
`orin-ledger.md` A27 and A30 name the owner as *"orin (… tegra cfg region of `main.rs`)"*, and
A8/A16/A19/A22/A23/A24/A39 all read `orin`.

Seven commits in this landing change `main.rs` **only** inside that region and nowhere else:
`5936f239`, `33dc7811`, `a5a62ffb`, `6f56eff8`, `4dc88314`, `f36cb82f`, `5fc5506a` (and `fc91eef9`,
which is additionally inside #3b's fold). Every enclosing item was read at the tip and carries
`#[cfg(all(target_arch = "aarch64", feature = "tegra" | "orinrender" | "deskcascade" | "supstate"))]`;
`deskcascade` is set only by `UNAOS_DESKCASCADE`, which implies `tegra` (`unaos/arroyo:1373`,
`deskcascade = ["desktop_firmware", "tegra_el0", "quarry"]` at `:1354`) and is unreachable from
`UNAOS_PIDESK`. rmbp 12's line-neutrality condition is met by all of them: every mid-file hunk is N→N,
and the only two growths are tail appends measured against the parent's line count — `6f56eff8` appends
at 8627 to an 8626-line parent, `f36cb82f` at 8869 to an 8868-line parent. `4dc88314`'s ledger row (A27)
records the same arithmetic independently: 8868 lines before and after, P7 index proofs, `kernel8.img`
identical.

`4dc88314` is the ruling's own named subject. The other six extend it **by class** — the class being
"orin-only cfg region of `main.rs`, mid-file hunks N→N, growth only at the tail." The class is what the
ledger's owner columns already assert; the announce asks rmbp to confirm it in one line, which closes
PANEL-REVIEW's F-1 permanently rather than per-arc.

### 3.3 The one hunk with no grant record, named rather than buried

`accca478` (QUARRYDOOR) chains its fixture from `crystal::selftest` with a **nine-line addition to
`unaos/crates/kernel/src/video/crystal.rs`** — a comment block plus
`#[cfg(all(feature = "quarry", feature = "witness"))] super::quarry::door_selftest();`. The executor's
own `PROGRESS.md` declares `video/crystal.rs` *"Orin lane, no ask needed"*, which contradicts this same
arc's rmbp **#4a**, **#4b** and `adb3b1cd` grants, all of which name `video/crystal.rs` as rmbp's file
and asked for it. The quarry half of the commit is covered by **#8**; **this hunk is not.** It is
knob-gated (`quarry` + `witness`), so it cannot move the knob-off `kernel8.img`, and `kernel8` was
measured identical across the fold: the exposure is governance, not bytes. **rmbp's grant on this one
hunk is the outstanding item at the announce.**

**One line disclosed beyond a granted set, recorded for completeness.** `5f8f392f` is a same-line
trailing comment in `video/prtscr.rs` whose wording **pi 7 supplied** as a PRTSCR3 fold condition
(14:3xZ). It was filed as one line beyond #3b's granted hunks and disclosed to rmbp, who then ran
GATE-APPEND on the containing tip `1fba879f` and reported it clean. **That is rmbp reading the line, not
rmbp granting it**; the ack step says so out loud.

**Also outstanding, both carried forward from PANEL-REVIEW and both MINOR.** **F-3** — `adb3b1cd`'s
grant line ends *"mod.rs/screen.rs under standing grants"*, and no document in `docs/dev/` quotes or
dates that standing grant; cite it or fold the two files into the enumerated list. **F-4** — pi 7's
MENUBAR "C ack" is recorded as *asked* in three places (`CLOSE-REPORT.md:45`, the commit body,
`MENUBAR-WINMENU.md:658`) and answered in none.

**Tally: 11 GRANTED · 1 granted by pi 7 with an unacked rmbp disclosure (`5f8f392f`) · 7
STANDING-GRANT / NOT-SHARED · 1 NO RECORD FOUND (`accca478`'s `video/crystal.rs` hunk).**

### 3.4 Open ack questions — the NO-RECORD list

These go to the peers at the announce, as written. **Silence is never consent**: each is answered, or
explicitly waived by the answering seat, before the merge.

| to | sha | what | why it is open |
|---|---|---|---|
| **rmbp** | `accca478` | the nine added lines in `unaos/crates/kernel/src/video/crystal.rs` (`crystal::selftest`'s tail: a comment block + `#[cfg(all(quarry, witness))] super::quarry::door_selftest();`) | no grant asked, none received. The executor declared the file orin-lane; rmbp #4a, #4b and `adb3b1cd` all treat `video/crystal.rs` as rmbp's. Knob-gated, `kernel8` identical — governance, not bytes |
| **rmbp** | `5936f239`, `33dc7811`, `a5a62ffb`, `6f56eff8`, `4dc88314`, `f36cb82f`, `5fc5506a` | **confirm as a class**, one line: the orin-only `#[cfg]` region of `main.rs` is orin's lane, no grant, line-neutral folds with growth only at the tail | rmbp 12's 03:0xZ ruling names `jd2_console_pump` and the `drag_route_tail` call specifically. Six of the seven extend it by class. Confirming the class closes PANEL-REVIEW F-1 permanently |
| **rmbp** | `5f8f392f` | acknowledge the one same-line comment disclosed beyond #3b's granted hunks | pi 7 authored the wording and granted it; rmbp's only recorded contact with it is a clean GATE-APPEND run on the containing tip — a check, not a grant |
| **rmbp** | `adb3b1cd` *(outside the 20; same class)* | cite or date the "standing grant" covering `video/mod.rs` and `video/screen.rs` | PANEL-REVIEW F-3: the phrase appears in the commit body; no document in `docs/dev/` quotes or dates it |
| **pi 7** | `adb3b1cd` *(outside the 20; same class)* | the MENUBAR "C ack" | PANEL-REVIEW F-4: recorded as *asked* in `CLOSE-REPORT.md:45`, the commit body and `MENUBAR-WINMENU.md:658`; answered nowhere. Silence is never consent |


**ANSWERED 2026-09-06 16:2xZ–16:4xZ over ccd (orin 17; the durable record is this section — rule below).** (a) `accca478` crystal.rs 9 lines: rmbp 14 **GRANTED RETROACTIVELY**, no change wanted; row condition: kernel8 identity holds for images without `UNAOS_QUARRY`/`witness` (`quarry` is a K8_FEATS arm); the defect named is an executor self-declaring a shared file its lane. (b) the seven orin-region `main.rs` commits: **CLASS CONFIRMED**, criterion restated by rmbp 14 — "a region of `main.rs` orin-only BY ITS ENCLOSING ITEM'S CFG, established by NAMING that item, needs no grant" (not line-neutrality; `main.rs` contributes no `panic::Location` to either default image, rmbp-ledger §D). Enclosing items, seat-measured on each sha's post-image and re-verified by rmbp 14 by brace balance: `6f56eff8` +2495 → `fn tegra_early_stop` @2029 `#[cfg(all(feature = "tegra", target_arch = "aarch64"))]` (body 2029..2718), +8819 → `fn tegra_shell_present` @8796 `#[cfg(all(target_arch = "aarch64", feature = "deskcascade"))]` (8801..8822); `f36cb82f` +8358 → `fn orin_render_service` @8242 `#[cfg(all(target_arch = "aarch64", feature = "orinrender"))]` (8242..8415), +8625 → `fn tegra_desk_cascade` @8552 `#[cfg(all(target_arch = "aarch64", feature = "deskcascade"))]` (8552..8626). rmbp 12's original ruling lived in a scratch file (`FOLD-NOTES.md`, not in git); rmbp 14 re-records it in their ledger. (c) `5f8f392f`: **ACKED**, nothing owed (one trailing comment, code first). (d) F-3: no citable standing grant for `video/screen.rs` exists ("a standing grant nobody can cite is a habit, not a grant" — rmbp 14); `video/mod.rs` is citable at `docs/dev/evidence/rmbp10/PANELREFUSE-REVIEW.md:5`. Correction to the list: `adb3b1cd` also changes `arch/x86_64/syscall.rs` (`crystal::key_escape` → `strip::key_escape` in x86's router) — **NO-RECORD in the x86 lane, granted on sight by rmbp 14 in this exchange.** (F-4) pi 7: MENUBAR condition C **was ACKED on 2026-09-06 to rmbp 12 first-hand** (23 non-neutral hunks across the four furniture files, all behind cfg'd-out `pub mod` decls; every hunk in a compiled file 1↔1; the one non-neutral hunk in a compiled file is `video/mod.rs @872,3`, tail of an 874-line file; `kernel8.img` byte-identical before/after per orin 16). Both seats had closed, so the ack existed only in pi's transcript. **Rule adopted from it (pi 7, 2026-09-06): the seat that RECEIVES a peer ack writes it into the row it covers, in the same commit, naming the acking seat and the scope — an ack that lives only in a ccd message dies with the seat and is not evidence for a landing.** GATE-APPEND on `963516c8` (rmbp 14): OK, 164 files, 4 controls fired, exit 0. F-1 is CLOSED on both sides.

### 3.5 Conditions carried into each fold

The grant is one half of the record; the condition attached to it is the other, and the conditions are
what a re-reader needs when a row is re-opened. Retained from the CLOSEDOCS draft, corrected where §3.1
corrected it.

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
| `video/prtscr.rs` (PRTSCR3) | **#3b** | **rmbp 13**, 14:2xZ; **pi 7** 14:3xZ | take the CONTENT DELTA, never a cherry-pick, and gate by file sha256 identity; pi's same-line `usb_backed()` invariant comment disclosed as one line beyond the granted hunks (§3.3); `strings` splits the refusal line at its em-dash — use `grep -a` |
| `src/power.rs`, `video/crystal.rs` (CRYSTAL) | **#4a** | **rmbp 13**, 14:1xZ; **pi 7** ack 16:4xZ | body path is `src/power.rs` (arch-neutral, NOT `arch/aarch64`); cfg prose exact `all(target_arch="aarch64", not(feature="pi"))`; pi's two edits — the `not(pi)` comment reads "no EL3 PSCI monitor in the Pi's CURRENT boot chain" (a boot-chain consequence), and the leg-4 behaviour change sits ABOVE the `action=stub` labelling |
| `video/menubar.rs` (CRYSTAL flush right, SO4) | **#4b** | **rmbp 13**, 14:1xZ | own commit from the patch; zero `cfg(` → arch-neutral, moves x86 bytes; rows A33/SO4 say UNFLOWN on rMBP; rmbp flies at 1920×1200 before J1 |
| `video/winmenu.rs`, `menubar.rs`, `crystal.rs`, `main.rs` (MENUBAR2) | **#5** | **rmbp 13**, 14:2xZ; **pi 7** ack 13:5xZ | `wm.rs` NOT touched (4 files); `main.rs` 8949/8949; A10 row names the CLASS and the two surfaced defects; verify the `crystal::selftest` chain order at the folded tip; pi's row condition — desktop_firmware-armed Pi images only, knob-off byte-identical and correctly so |
| `video/wm.rs`, `video/fbcon.rs`, `video/instgui.rs`, `video/pulsewin.rs`, `video/quarry/live.rs`, `main.rs` (WINID) | **#6** | **rmbp 13**, 14:1xZ (both halves + the Pi-drain `main.rs` extension); **pi 7** (b) 14:2xZ | WINID-video.patch = 9 hunks / 5 video files, all verified N→N or tail (`git show --stat b19b2865`); register the sixth holder or name it unregistered with a reason; the row states what a second `orin_wm1` call after a close does; pi's S4/S7 clause relayed. **`video/dock.rs` is NOT in this family** — it is #2/B19's file |
| `video/wcg.rs` (WINID2) | **#6 addendum** | **rmbp 13**, 14:3xZ; condition closed 14:5xZ | sited on `wcg::begin`, **not** at the store line (`seam_glyph_note` runs in print context under a no-lock/no-print contract) — rmbp amended **B23**; gate `all(witness, any(x86 wc, aarch64 desktop_firmware))`; knob-off `kernel8` identity is **VACUOUS** (pi 7: `pub mod wcg;` is `cfg(witness)`, absent from the knob-off image) |
| `fs/vfs.rs`, `video/shell.rs` (ROOTFS) | **#7** | **rmbp 13**, 14:2xZ | add `target_arch = "aarch64"` to the `shell.rs` same-line cfg, statement BEFORE the line's first `//`; `sdmmcroot` not Pi-eligible (SR1 class) |
| `main.rs`, `arch/x86_64/syscall.rs`, `video/quarry/live.rs`, `unaos/scripts` (KEYDOORS-FIX) | **#8** | **rmbp 13 — GRANTED, a/b/c/d**; Pi behaviours **pi 7 acked** (`ROUND-QUEUE.md` LANDED; `orin-ledger.md` A10) | fold only the F0/F1/F2/F3 `main.rs` hunks under #8; the aarch64 `drag_cancel("focus-key")` twin goes to rmbp as a patch, theirs to cut or accept; at fold the A10 row takes the F0 note. **The `video/crystal.rs` nine lines in `accca478` are outside this grant** (§3.3) |

**Lane exposure the announce must carry** (TRUNKPREVIEW §2, unblocking): the arc touches
`arch/x86_64/syscall.rs`, `drivers/xhci/mod.rs`, `fs/vfs.rs`, `main.rs`, `power.rs`, `shell.rs` and
fifteen `video/` files. The xhci grant is already in its commit body; the peer-ack step confirms the
grant record for the `video/` family and `arch/x86_64/syscall.rs`, and answers §3.4.

---

## 4. TRUNKPREVIEW 1 at `8d9627f5` (throwaway; nothing landed) — **superseded by §5**

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

**This preview did not transfer, and it was not asked to.** It ran at `8d9627f5`; the arc advanced 37
commits past it. **§5 is the preview of record** — it re-derives every one of these facts at the final
arc tip `963516c8`. Two observations carried forward from here, neither a blocker: QEMU-green is not
hardware-green (`kernel8-test` is the Pi QEMU battery at 640×480 defaults; the Orin evidence is
render8); and trunk currently holds only one arch ledger — `docs/dev/OS/orin-ledger.md` — since
`ledger-check.sh` SKIPs a missing ledger with a line, so trunk's ledger surface is orin-only plus the
shared files until pi and rmbp land theirs.

---

## 5. TRUNKPREVIEW 2 at the final arc tip `963516c8` — the preview of record

Executor TRUNKPREVIEW2 (orin 17). Full report `~/unaos-bench/scratch/orin16/trunkpreview2/PREVIEW-2.md`;
logs in the same directory. Run on a **detached HEAD** in
`~/unaos-bench/scratch/orin17/wt-trunkpreview2` — no push, no stash, no commit to `hw-jetson`, `main`
never checked out or written, and the shared trunk worktree `../UnaOS` verified still at `f49ea1e7` by
`git worktree list`. Preview merge commit **`d9bc1636`**; `git branch --contains d9bc1636` → empty.

> ## Verdict: LANDABLE — 0 conflicts, shape PASS, every battery leg exit 0 except one x86 `./arroyo test` run whose single `[ptrdead]` red matches FIXTURE_FLAKES Class 3 on its own discriminator (`fpop12=1`, non-zero) and cleared 3/3 on re-run of the identical binary; the merged tree is byte-identical to `hw-jetson`, so the merge introduces nothing.

### 5.1 Conflicts, and what the merge brings to trunk

`git merge --no-ff --no-commit 963516c8` → "Automatic merge went well; stopped before committing as
requested", exit **0**. **Conflict count 0** — `--diff-filter=U` 0 files, `git ls-files -u` 0 entries.
Nothing was hand-resolved. The marker scan returns the same two benign files as preview 1
(`S7-STEP1-v2-resolve.py`'s marker literals; `LEDGER.md:69`'s P8 hazard row quoting the guard):
**real conflict markers in tracked source, 0.**

`git diff --shortstat f49ea1e7 d9bc1636` → **107 files changed, 46 008 insertions(+), 634 deletions(−)**;
64 of the 107 are under `docs/dev/evidence/`. The shared/other-lane files in the remainder — the subject
of §3 — are `arch/x86_64/syscall.rs`, `drivers/xhci/mod.rs`, `fs/vfs.rs`, `main.rs`, `power.rs`,
`shell.rs` and fifteen `video/` files.

### 5.2 Merge-shape check on the preview merge (LAWS §Code and history)

| # | fact | command | result |
|---|---|---|---|
| 1 | two parents | `git log --pretty=%p -1 d9bc1636` | `f49ea1e7 963516c8` — trunk tip **and** arc tip. **PASS** |
| 2a | diff against arc parent | `git diff 963516c8 d9bc1636 \| wc -l` | **0** |
| 2b | trunk original work since base | `git log --no-merges --oneline 671a0334..f49ea1e7 \| wc -l` | **0** (all three trunk commits are `Merge hw-jetson:`) |

**SHAPE PASS.** The zero diff at 2a is safe **precisely because** 2b is empty: trunk contributed no
original content since the base, so a merged tree identical to the arc parent loses nothing. Without
2b, a zero diff against a non-ancestor parent is indistinguishable from wholesale loss of trunk-only
content. **These are the same two commands §6 re-runs on the real merge; the preview does not satisfy
them.**

### 5.3 Battery on the merged tree `d9bc1636` — exit codes

Legs ran **sequentially**, never concurrently; the driver is `run-battery.sh` in the log directory, and
every exit code was captured by the driver (`echo "EXIT=$rc"`) as the last line of its log. **No leg is
scored from printed text alone.**

| leg | exit | evidence | log |
|---|---|---|---|
| `./arroyo check` (both arches) | **0** | 0 red (`^error[` / `^error:` = 0); x86_64 OK, aarch64 OK, bootloader OK; **kernel cfg-gated coverage OK (54 legs)**; 75 ✅; **GATE-KNOB OK** — 162 features declared, 161 named by a cfg, 0 phantom, 0 dead, 0 trailing-comment cfg; **GATE-LEDGER OK (107 rows)** | `check.log` |
| `./arroyo test` (x86 headless) — **run 1** | **1** | **RED, reported not hidden.** 73 PASS/witness lines, **one distinct `-> FAIL`**: the `[ptrdead] backlog` rollup. §5.4 | `test-x86.log` |
| `./arroyo test` — re-runs 2, 3, 4 | **0**, **0**, **0** | 0 FAIL each; `[ptrdead] … fpop12=0 fpop3=0 -> PASS`. **0 `Compiling` lines in all three — the identical binary that reddened** | `test-x86-r2/r3/r4.log`, `ptrdead-r2/r3/r4.txt` |
| `./arroyo test-arm 60` | **0** | 0 FAIL; serial at `unaos/target/serial-arm.log` | `test-arm.log` |
| `./arroyo kernel8` | **0** | image built; sha in §5.5 | `kernel8.log` |
| `./arroyo kernel8-test` — run 1 | **0** | **MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 10 227 lines scanned**; 117 REQUIRE + 133 FORBID printed, **0 ❌**, **0 SO7 hits**; 72 s | `kernel8-test-1.log` |
| `./arroyo kernel8-test` — run 2 (quiet-box) | **0** | **MBENCH PASS — 119/119, 0 forbidden hit(s), 12 085 lines scanned**; **0 ❌**, **0 SO7 hits**; 71 s | `kernel8-test-2.log` |
| `UNAOS_LEDGER_STRICT=1 unaos/scripts/ledger-check.sh` | **0** | `GATE-LEDGER: OK — 107 rows in 2 ledger file(s) + RULINGS: ids unique, status ∈ enum, owners known, cross-refs resolve, shas exist, evidence in git and anchored, rulings live or superseded-by a real R<n>`. **107 is the number this landing cites**; §5.6 qualifies what "strict" means on this tree | `~/unaos-bench/scratch/orin17/ledger-strict-preview.out`, `ledger-check.log` |
| k8 reachability (rmbp 14's `k8-reach`, run on the preview tree) | ✅ | **115 knobs: 11 armed, 104 registered unarmed**; §5.7 | rmbp-side, §5.7 |
| widened cross-ref resolver (`xref-widened.py`, over the merged ledgers) | **0** | **LATENT-RED = 0** — 94 distinct ids pooled across `LEDGER.md` (38), `orin-ledger.md` (43), `RULINGS.md` (13); 19 cross-refs matched, **0 unresolvable**. Strictly stronger than the shipped gate | `latent-red.log` |

**Nine of the ten battery runs exit 0. The one exit-1 is x86 run 1**, treated in full immediately below.

### 5.4 The x86 red — `[ptrdead]`, and why it is FIXTURE_FLAKES Class 3

The witness, verbatim (`unaos/target/serial.log:922`):

```
[ptrdead] backlog whole=false nodrop=true order=false pushed=192 entries=0 travel=(0,0) folded=192 dropped=0 fpop12=1 fpop3=2 quiesced=false -> FAIL
[ptrdead] order detail: got=[Some(Mouse { x: 2, y: 0 }), None, None, None] fpop=2 fpush=-1 quiesced=false
```

`docs/dev/FIXTURE_FLAKES.md` **Class 3 — "PTRDEAD's backlog leg loses its coalesced entry to a FOREIGN
consumer"** names **`fpop12` as the discriminator** and says to read it before anything else:
`foreign12 = (evq_pops() - pop0) - entries` counts pops made by a consumer *other than this leg* while
the leg held the queue. **`fpop12 = 1` — non-zero. That is the class.** The entry also states the shape
that would NOT be this class and *would* be a regression: `fpop12=0` with `whole=false` means the
coalescing itself broke. That is not what happened here.

Class 3's four capture items, answered:

| # | asked | captured |
|---|---|---|
| 1 | the full `[ptrdead] backlog` line | `fpop12=1 fpop3=2 entries=0 folded=192 pushed=192` (quoted above) |
| 2 | the `order detail:` line if `order` also went red | `order=false` here, so captured: `got=[Some(Mouse { x: 2, y: 0 }), None, None, None] fpop=2 fpush=-1` |
| 3 | what else was running on the host | No other `arroyo`/`cargo`/`qemu` process — the battery is strictly sequential and `pgrep -a -f 'arroyo\|cargo\|qemu'` was empty. But **the red leg compiled the x86 kernel inside itself**: run 1 took 65 s with `Compiling` lines present; the three greens took 21 s each with **0 `Compiling` lines**. A build was racing the QEMU run *in the same leg* — Class 3's stated trigger |
| 4 | is a re-run on an idle host clean, and how many | **3 of 3 clean**, `fpop12=0 fpop3=0 … -> PASS` each time. Host loads at re-run start: 1.82 / 2.21 / 2.37 |

**Same binary across the red and the three greens** — 0 `Compiling` lines in all three re-runs, so cargo
rebuilt nothing between the FAIL and the PASSes. Only host conditions differed.

Two honest deviations from the corpus's worked example, recorded so the entry can be **widened** rather
than silently matched: the example has `order=true` and `folded=191` (one short of `pushed`) where this
run has `order=false` and `folded=192` (equal to `pushed`) — the entry explicitly anticipates the order
leg also reddening, and `folded=192 entries=0` is the same mechanism (the foreign consumer popped the
*coalesced* entry after folding finished) rather than a folding defect; and the example has `fpop3=0`
where this run has `fpop3=2` — a second non-zero foreign-pop counter, same direction as `fpop12`.

**This red is not introduced by the merge.** `git diff 963516c8 d9bc1636` is **0 lines** (§5.2), so the
merged tree is byte-identical to `hw-jetson`'s. Whatever the x86 leg does here, it does identically on
`hw-jetson` today, unmerged. The landing cannot create or worsen it.

**Disposition per the corpus: WATCH — not fixed, and it does turn the x86 gate red on its own.** That
stands; this preview does not close it. It adds a fourth observation to the class (the first with
`order=false`, the first with `fpop3` non-zero, and the first where the racing build is the leg's own
compile rather than a concurrent `arroyo`). **§8's trunk battery can hit it again**; a single red does
not convict, and a re-run habit is not the answer either (rmbp 13, B26).

### 5.5 `kernel8-test` against the floor, SO7, and the artifact

Floor from `unaos/scripts/specs/pi4-regression.spec` in the merged tree: `grep -c '^REQUIRE'` → **117**,
`grep -c '^COUNT'` → **2**, `grep -c '^FORBID'` → **133**; `unaos/scripts/mbench.py:135` adds
`DEFAULT_FORBIDS = [r"-> FAIL", r"FAIL ::", r"PANIC"]` → 3 more.

| | floor | run 1 | run 2 (quiet-box) | verdict |
|---|---|---|---|---|
| required witnesses | 117 REQUIRE + 2 COUNT = 119 | **119/119** | **119/119** | **MET, both runs** |
| FORBID checks | 133 + 3 default = 136 | **0 hits** | **0 hits** | **MET, both runs** |
| ❌ marks | 0 | **0** | **0** | MET |
| lines scanned | — | 10 227 | 12 085 | — |

**SO7 did not fire in either run.** SO7 records `kernel8-test` reddening intermittently under host build
load with content-bearing `[wc-d] verify … bad_cache=N bad_ram=N` and `[wc-g] … RACE-PRESENT` hits.
Scanning both logs for `[wc-d].*bad_cache`, `RACE-PRESENT` and `RACE-BLIT`: **0 hits in run 1, 0 in run
2.** No `[ptrdead]` lines in either (that fixture is x86-only). **The two runs are the SO7 discriminator
and both came back clean** — the quiet-box run §8's condition 2 owes is therefore on the record *for the
preview tree*, and §8's own runs must repeat it on the merge commit.

| artifact | sha256 | size |
|---|---|---|
| `unaos/target/pi_baremetal/kernel8.img` | `ed258846c9d20887fe70d778f70ab6ea3ce6e5da71caca7f336d79512be2826e` | 1 817 704 B |

Taken with `sha256sum`, **not** from the leg's own summary line, which prints the 64 MB flashable
image's sha rather than `kernel8.img`'s. Per the standing rule, a boot is scored against the loaded
image, not a card sha.

### 5.6 The ledger gate, and what "strict" does and does not mean on this tree

`UNAOS_LEDGER_STRICT=1 unaos/scripts/ledger-check.sh` on the preview merge `d9bc1636` → **exit 0, 107
rows** in 2 ledger files + RULINGS (`~/unaos-bench/scratch/orin17/ledger-strict-preview.out`). **107 is
the number this landing cites.**

Two qualifications, both stated rather than assumed:

1. **The arc's own `ledger-check.sh` does not yet read `UNAOS_LEDGER_STRICT`.** The strict/deferred split
   is rmbp's `44c7887b`, which is **not an ancestor of `963516c8`** (`git merge-base --is-ancestor` →
   false; the 165-line script contains no `STRICT`, `DEFERRED` or `cross-branch` token). On this tree the
   variable is inert, so the 107-row green is the plain gate's. That is the same check rather than a
   weaker one — there is nothing in these ledgers for strict mode to have deferred — but the claim is
   "the gate is green at 107 rows", not "strict mode was exercised".
2. **rmbp 14's finding stands and is why that distinction matters.** Non-strict runs are green *by
   construction* off trunk, because a track branch's cross-branch refs are deferred rather than resolved.
   rmbp's own logs show both halves of the shape on `hw-rmbp`: `GATE-LEDGER: RED — 1 finding … B22
   cross-ref → SO6 does not resolve` under strict, and `DEFERRED — 1 cross-branch cross-ref(s); NOT
   findings` without it. Once rmbp's resolver split lands on trunk, every deferred ref becomes a red
   automatically at the landing (§9). The stronger check available today is the widened resolver
   simulation (§5.3): **LATENT-RED = 0** over 94 pooled ids and 19 refs, which does not defer anything.

### 5.7 k8 reachability

rmbp 14's `k8-reach` gate, run over the preview tree: **✅ 115 knobs — 11 armed, 104 registered unarmed.**
The 104 are *registered* and marked TODO, not unregistered: the gate's contract is that every knob is
accounted for, not that every knob is exercised. The script and its registry are rmbp's
(`~/unaos-bench/scratch/rmbp14/gored/scripts/k8-reach.registry`) and the result line is theirs —
**there is no orin-side log of this run in this seat's scratch**, so it is recorded here as a relayed
result with its provenance named, and the announce is where rmbp confirms the number against their own
run.

### 5.8 What the preview does **not** settle

- **It is not the landing.** The battery re-runs on the merge commit in `../UnaOS` (§8) — not on the
  preview, not on the arc tip.
- **Its `ls-remote` freshness does not carry.** `main` was `f49ea1e7` and unmoved at that run; the
  announce and the merge each run their own check in the same turn (§7).
- **QEMU-green is not hardware-green.** `kernel8-test` is the Pi QEMU battery at 640×480 defaults. The
  Orin evidence is render8, and Orin/Pi bench flights are a bench matter, not a landing gate.
- **The F-1 record is a peer question, not a preview finding.** §3 resolves it against the round ledger
  and the tracked ledgers; §3.4 is what remains for the peers.

---

## 6. Merge-shape rule (LAWS §Code and history, pi 6 at LANDING-2 `d11cd56e`) — PASS

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
| 1 | two parents | `git log --pretty=%p -1 c7407753` | **PASS** — `f49ea1e7 c5048fe6` |
| 2a | diff vs arc parent | `git diff c5048fe6 c7407753 \| wc -l` | **PASS** — `0`; tree `c6ce5ef4` both (pi 7: main^{tree} == merge-base^{tree} `1eb76454`; rmbp 14: `git merge-tree --write-tree` = `c6ce5ef4`) |
| 2b | trunk original work since base | `git log --no-merges --oneline 671a0334..f49ea1e7 \| wc -l` | **PASS** — `0` (the three commits beyond the base are merges OF hw-jetson: orin 13, 13b, 14) |

Both halves already PASS on the preview merge `d9bc1636` (§5.2). **That is not a substitute**: these
rows are the same two commands re-run on the real merge commit.

---

## 7. Landing-race rule, and the landing sequence — DONE

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

**Landing pending — the order is fixed and no step may be taken out of it.** (1) **Peter pushes the
flight commit** to `origin/hw-jetson`, so the tip the announce names is fetchable by both peers; (2) the
seat **announces** over ccd with a fresh `ls-remote` run that same turn, carrying §3.4's open ack
questions; (3) **peer acks** come back from rmbp 13 and pi 7 — at least one is required, both are asked,
and §3.4's `accca478` question is answered or explicitly waived by rmbp; (4) a **fresh `ls-remote`
immediately before** the merge; (5) the **`--no-ff` merge in `../UnaOS`**, arc into trunk; (6) the
**trunk battery** on the merge commit (§8), with §6's two shape commands. Only after (6) does the DRAFT
banner come off this file.

| step | state |
|---|---|
| adversarial panel on the final tip | PANEL-REVIEW (orin 16, code at 963516c8; the two later commits are docs) + TRUNKPREVIEW2 §5 |
| Peter's push of the flight commit | `hw-jetson c5048fe6` on origin at 16:39Z (host `ls-remote`) |
| `ls-remote` at the announce | 16:39:11Z — `hw-jetson c5048fe6 · hw-pi4 f25f1601 · hw-rmbp 72fd1de3 · main f49ea1e7` |
| peer ack #1 (rmbp 14; rmbp 13 was archived) | **GRANTED ~17:2xZ** — their `ls-remote`: `c5048fe6 · f49ea1e7 · bfcea17e · 36deacd6`; computed, not reviewed: `merge-base --is-ancestor f49ea1e7 c5048fe6` NO (real merge), 93 commits, `git merge-tree --write-tree f49ea1e7 c5048fe6` = `c6ce5ef4…` == `c5048fe6^{tree}` exit 0; `ledger-check` 111 rows exit 0 AND `UNAOS_LEDGER_STRICT=1` exit 0 on the tip; GATE-APPEND on 963516c8 OK (164 files, 4 controls). NOT verified by them: check/test/test-arm/kernel8/kernel8-test. §3.4 answered (a)–(d), see there. Their ledger row B37 |
| peer ack #2 (pi 7) | **GRANTED 16:40:09Z** — their `ls-remote`: `c5048fe6 · bfcea17e · 36deacd6 · f49ea1e7`; proof by tree object: `main^{tree}` == `merge-base^{tree}` (`1eb76454…`), so the merged tree is hw-jetson's (`c6ce5ef4…`) by construction; every commit they granted verified in range by ancestry (`99c153ca`, `5f8f392f`, `0f6a12d2..6aef5227`, `fdbaeb87`, `c5048fe6`). NOT checked: any gate run, metal, the knob-off image, behaviour. F-4: acked (§3.4). Their observation: trunk carries no pi/rmbp work yet, so their first landings are real merges against 93 orin commits |
| `ls-remote` immediately before the merge | 16:42:01Z — `main f49ea1e7` unmoved (hw-pi4 had moved to `bfcea17e`, irrelevant to this merge) |
| `--no-ff` merge commit in `../UnaOS` | `c7407753`, 16:42Z, 94 commits (93 + merge); pushed by Peter, on origin at 16:50Z |
| trunk battery on the merge commit (§8) | **GREEN**, 16:42–16:53Z |

---

## 8. Trunk battery on the merge commit `c7407753` — GREEN

Runs in `../UnaOS` on the merge commit, **not** on the preview (§5) and **not** on the arc tip. No number
below is filled in.

| leg | exit | evidence |
|---|---|---|
| `./arroyo check` (both arches) | **exit 0** | 0 ❌; GATE-KNOB OK; GATE-LEDGER OK |
| `./arroyo test` (x86; the `UNAOS_WC=1` form ran on the folded hw-jetson tip `d2e36929` in the same hour: exit 0, `wc` in the banner) | **exit 0** | `[ptrdead] … fpop12=0 fpop3=0 -> PASS` — no Class-3 flake this run |
| `./arroyo test-arm 60` | **exit 0** | — |
| `./arroyo kernel8` + `./arroyo kernel8-test` ×2 | **exit 0 / 0 / 0** | `MBENCH PASS — 119/119 required witnesses`, 0 forbidden, both runs; SO7 hits 0/0; host NOT quiet (nine executors and their gates live) — a quiet-box run stays owed to rmbp's SO7/B26 fixture item |
| `unaos/scripts/ledger-check.sh` | **exit 0** | 111 rows, 2 files + RULINGS. ABSENT on trunk at this tip, not green: GATE-APPEND (`append-position.sh`), GATE-K8REACH (`k8-reach.py`) and the ledger STRICT/DEFERRED split — they arrive with rmbp's J1 (rmbp 14) |
| `kernel8.img` sha256 | `ed258846c9d20887…` (1817704 B) | `sha256sum` on `../UnaOS/unaos/target/pi_baremetal/kernel8.img` after the battery; TRUNKPREVIEW2's build of the identical tree read `ed258846c9d20887…` — a per-tree chain, compare within one chain only |

**Four conditions on this table before it can be quoted.**

1. **The `kernel8-test` denominator moves to 120 when pi lands** — `hw-pi4`'s `f25f1601` spec has REQUIRE=118
   (the CHROMEBAND leg, `[wc-b] rollup … amp=1.00x`, toothless on Pi but it must be EMITTED). Checked on our
   own pidesk capture: 53 occurrences, so 120/120 will hold.
2. **SO7 is open and this leg is its subject.** `kernel8-test` reds intermittently under host build load with
   content-bearing hits, at loads as ordinary as ~3.9. **A quiet-box run is OWED before this battery is used
   as a gate.** Preview 2 ran that pair on the merged tree and got 0 SO7 hits in both runs (§5.5) — that is
   the discriminator answered *for the preview tree*, and it must be repeated here. A single red does not
   convict (pi's rule) — but a re-run habit is not the answer either: if the baseline rate is non-zero on a
   quiet box, the fixture becomes load-independent or the flaky leg gets an explicit allowlist (rmbp 13, B26).
3. **The x86 `[ptrdead]` Class 3 flake can red this table's `test` leg** (§5.4). It is a pre-existing
   condition of the arc, not something the merge introduces — the merged tree is byte-identical to
   `hw-jetson`. If it fires, capture Class 3's four items, re-run on an idle host, and record **both**,
   exactly as §5.4 does. Do not re-run it into silence.
4. **Gate runs longer than ten minutes die under the Bash tool cap with no result and exit 1.** That is not a
   red leg. Run the chain under `nohup` and poll a RESULT file.

---

## 9. Docs reconciliation owed at the landing (F-1)

- `docs/dev/OS/08_VIDEO/screenshot.md` §10/§11 were **written on `hw-rmbp`** (one-sided +70 at J1). **Do not
  write them here** — reconcile at the merge.
- The `hw-rmbp` copy of `docs/dev/LEDGER.md` does not yet carry `S28`–`S32`; that union is the landing seat's,
  by the standing rule (keep both tracks' additions).
- `SO6` is intentionally absent from `LEDGER.md`: the `arroyo`-from-repo-root false red is rmbp's **B22** and
  is cross-referenced, not re-filed.
- Cross-tree cross-refs (P14): an `→ X` in an id cell means the gate must resolve it. rmbp's resolver split
  (`44c7887b`) makes seat-prefixed ids (SR/SO/SP) DEFERRED — printed and counted — and **STRICT at the
  landing**. That split is **not in this arc's tree** (§5.6), so it arrives with rmbp's own landing; every
  deferred ref must resolve once the ledgers merge.
- **§3.4's ack questions are a docs deliverable too.** Whatever rmbp and pi answer at the announce is written
  back into §3 and into `docs/dev/OS/orin-ledger.md`'s owner columns, so the next arc inherits the record
  rather than re-deriving it.
</content>
