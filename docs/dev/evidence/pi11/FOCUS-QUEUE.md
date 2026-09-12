# pi 11 — the aarch64 (Pi 4) FOCUS QUEUE, archived into the tree

**What this is.** The verbatim contents of `~/.claude/plans/unaos/queue/pi-focus-queue.md` as of
2026-09-11, 2830 lines / 218,143 B, sha256 `83c8ae1001dfa7391c1c9b6273674522d9524cfd2931777fe55e363d39d0795e`. It was written by
pi 8 on Peter's instruction (*"queue pi work items for your focus time"*) and appended to through
pi 9 and pi 10. **Nothing below this header is edited, reordered or summarised** — the file's own
standing rule is APPEND, DO NOT REWRITE, and an archive that rewrites its subject is not an archive.

**Why it moved.** It lived outside version control. `git -C ~/.claude/plans rev-parse --git-dir`
answers *fatal: not a git repository*, so every word in it — including three of Peter's verbatim
rulings that exist in no other file in this repo — was one `rm` from gone. rmbp solved the same
problem the other way and their solution is already in the tree, so this adopts their corpus rather
than inventing a parallel one: `docs/dev/evidence/rmbp16/FOCUS-QUEUE.md`, versioned, superseding
`../rmbp15/FOCUS-QUEUE.md` each round. Ledger row: `LEDGER.md` SP18.

**What is authoritative now.** This file is a RECORD, not a tracker. The work items it held that
existed nowhere else were migrated to `docs/dev/LEDGER.md` on 2026-09-11 as **SP10-SP18**, each
re-derived in this tree at `1b54c03f` rather than copied — the queue's own line numbers and floors
are up to 94 commits stale and its header says so. Read the ledger for what is owed; read this for
why, and for the reasoning, retractions and peer exchanges behind it.

**The loose copy still exists and is NOT deleted.** Peter's yes was conditional
(*"no data will be lost correct, just the redundancies will be deleted? if so yes"*), R20 forbids
trashing work, and rmbp 19 held the delete pending his word in pi's own session. This archive makes
the delete safe; it does not make it authorised.

⚠ **Every number below is stale by construction.** The file's own warning applies to itself:
*"whoever reads it at the focus turn must re-source any number before acting on it."* Floors,
line numbers and shas were true in the tree and at the moment each entry names.

---

# PI FOCUS QUEUE — the ordered work list for pi's next FOCUS turn

> **Created 2026-09-06 by pi 8 (support seat) on Peter's instruction: "queue pi work items for your
> focus time."** Until now this list lived only in the running baton, which dies with the seat.
> This file is the interim home; **it is NOT the tracker.** `docs/dev/LEDGER.md` is, and
> `docs/dev/OS/pi-ledger.md` becomes pi's arch tracker at the first focus turn (LEDGER `S24`, deferred
> by Peter to that turn). **At the focus turn: create `pi-ledger.md`, lift the rows, and this file
> becomes a pointer.**
>
> **Every value below was derived in THIS tree, this turn** (`hw-pi4` `059e04db`, 2026-09-06 14:34 MDT).
> Re-derive before acting — pi 7's round moved values ~15 times.
> Never copy another tree's number: baselines and gate floors are per-tree chains.
>
> **APPEND, DO NOT REWRITE.** New items arrive from peer exchanges during support rounds; each one
> lands here the turn it is found, with the command that proves it.

## STATE AT WRITING — verified this turn, not relayed

    hw-pi4    059e04db  == origin, tree clean, 0 unpushed.   PI OWES NOTHING TO PETER'S PUSH QUEUE.
    main      c7407753     pi is 94 BEHIND / 36 AHEAD  (git rev-list --count, both directions)
    hw-jetson 98ffd63d     hw-rmbp 5c3dbb7e  (rmbp MOVED from the baton's 141cc728)

Command, both halves, run it again before you trust it:

    flatpak-spawn --host git ls-remote --heads origin      # bare in-sandbox ls-remote dies on publickey
    git rev-list --count HEAD..c7407753 ; git rev-list --count c7407753..HEAD

---

## 1. THE TRUNK FOLD — 94 COMMITS. EVERYTHING ELSE IS SMALLER.

**Trunk's tree is byte-identical to hw-jetson's** (pi 7 verified `main^{tree} == c5048fe6^{tree}`),
so **trunk carries no pi and no rmbp work at all** — the orin landing was conflict-free *because* it
was a pure orin line. Pi's fold meets 93+ commits of orin change, much of it in `main.rs` regions pi
owns. **The cheap merge was the expensive one deferred.** Budget a milestone for it, not a step.

**Gate after the fold:** `./arroyo check` (both arches) + `test-arm` + `kernel8-test`.
Run `check` from `unaos/`, **not the repo root** — from the root the knob→builder probe red-lines
(`${BASH_SOURCE[0]}` stops resolving after arroyo's internal `cd`; orin's SO6, owner rmbp).

**Spec floor, derived HERE this turn — denominator = REQUIRE + COUNT:**

    grep -c '^REQUIRE' unaos/scripts/specs/pi4-regression.spec   # 118
    grep -c '^COUNT'   unaos/scripts/specs/pi4-regression.spec   #   2   → floor 120

DUPGUARD makes it **121** (one new REQUIRE; its `COUNT 26→27` is a VALUE bump, not a new COUNT line).
hw-jetson's floor is 119→120. **Two different 120s. Never quote another tree's number.**

**60 s TRUNCATES under contention — re-run at 420 and say so.** But `[wc-g] RACE-PRESENT` /
`[wc-d] bad_cache==bad_ram` are NOT that family (SO7): content-bearing, captures reach the tail, and
the mechanism is `fbcon`'s glyph raster repainting under the compositor's checksum bracket
(`wcg.rs:412`, WCGSEAM, WCGWIN1 §6.13 — LIVECON is NOT the fix). **It fires at load ~3.9**, not 10.84,
so pi's battery is routinely exposed.

### 1a. THE ATTRIBUTION TABLE — what may and may not move pi's baseline
Not a queue of checks: **it is what gives an unexplained delta a suspect list.** orin measured these
on ITS chain. **Re-derive pi's own values; never copy these.**

    8ff7c1d1  held through MENUBAR · CLICKDEAD · S4/WINID · MENUBAR2 · SERIALRX-DEDUP · BSPRUN
    3f14337c  from PRTSCR3 onward (prtscr.rs is UNCONDITIONAL — the one change that moves it)
              unchanged again through WINID2 · KEYDOORS · JV1 · VIRTPREEMPT
    CRYSTAL is the pidesk DESKTOP image — a SEPARATE chain; never compare it against these.

**Exactly one change in the whole fold moves the knob-off image, and it is PRTSCR3. A move anywhere
else is the signal.**

**Byte-identity discipline: POSITION + LINE-NEUTRALITY, never a `#[cfg]`.** The ONE exception: a
cfg'd-out `pub mod` means the file is never lexed, so its internal hunks are free (orin's MENUBAR: 23
non-neutral hunks, image identical). A cfg'd-out BLOCK is the opposite. **Compare `kernel8.img`,
never `kernel.elf`.**

### 1b. TWO `cmp`s PI OWES, both post-fold, both on PI'S TREE
- **MENUBAR** — *should* be byte-identical (23 non-neutral hunks all in furniture files behind
  cfg'd-out `pub mod`, never lexed). **13 non-neutral hunks in `pulsewin.rs` is where to be wrong
  loudly.** The never-lexed reasoning is an ARGUMENT until the `cmp` runs.
- **CLICKDEAD** — should move the image **exactly once**, for the recorded reason (two ungated BSS
  atomics + one relaxed increment, the `MOUSE_REARM_COUNT` pattern at `drivers/xhci/mod.rs:2430`).

### 1c. OPEN QUESTION PI 7 ASKED AND NEVER GOT ANSWERED
**What artifact was `b5c0a3a1…`, the S7 baseline pi accepted?** It does not connect to the
`d73a8981…` chain — likely an ELF or a per-function digest, not `kernel8.img`. pi 7 enforced
"compare kernel8.img not kernel.elf" all night and then accepted a baseline without pinning its
artifact. **Close it: ask orin, or re-derive.**

---

## 2. REGISTRY-FULL `FORBID` for `pi4-regression.spec` — READY, ONE LINE
**Precondition MET and measured by orin:** `[wm] winid-register REFUSED` = **0** across two
`UNAOS_PIDESK=1 kernel8-test` runs (6 holders + fixture against a ceiling of 8).
Adding it moves the floor 120 → 121 with DUPGUARD; recount, do not assume.

## 3. `[wc-d] moved=` RULE — **BLOCKED ON PURPOSE. DO NOT TIGHTEN YET.**
The verifier already emits `moved=` (`wm.rs:6682`; `wm.rs:6192` defines it as the reference moving
under it) and the spec never consults it — `:576` REQUIRE and `:577` FORBID both ignore the field.
**Tighten only after the fixture emits a distinct verdict (`-> RESAMPLE`) for `moved != 0`.**
Tightening first manufactures a red with no defect. Mechanism: SO7.

## 4. SP3 — ONE-LINE HEADER FIX in `drivers/emmc2.rs`. **RE-VERIFIED HERE THIS TURN.**
`emmc2.rs:5-7` says *"no DMA, and NO writes (the seam in `drivers::block` refuses a write on this
backend)"*. Both halves of the write claim are false since U9:

    sed -n '1195,1200p' unaos/crates/kernel/src/drivers/block.rs   # :1197 BACKEND_SD -> emmc2::write_block_512
    # and emmc2.rs:798 defines write_block_512

**This is the expensive direction of stale prose** (S17/S28 make you hunt for what is gone; this makes
you believe a capability is ABSENT when it is present — nobody looks twice at a door they were told is
locked). **It is live on this very queue: item 7 (S14's closer) needs a WRITABLE card.** A seat reading
this header would conclude Print Screen to the Pi's card is impossible and abandon available work.
Fix when the file is next touched. No code.

## 5. DOCUMENT THE `COUNT 26 ::` COUPLING — load-bearing and undocumented
`pi4-regression.spec:262` is a **GLOBAL census of `::` witness lines**, so *any* seat adding *any*
`::` witness *anywhere* must bump it or pi's spec reds. orin's DUPGUARD executor hit it blind.

## 6. GATE-K8REACH — the 104-row pass (rmbp 14's ask, no deadline)
104 unarmed knobs seeded `TODO`; 65 have a Pi-live cfg site, 39 have none.
`scripts/k8-reach.py --evidence <KNOB>` turns each row into a command.
⚠ **Every `NA` must cite the `--evidence` output, not a reason string** — a prose pass inherits the
classifier's blind spots exactly (`nvidia-kepler`'s arch-neutral site, `rastmc`'s x86-gated callee).

## 7. S14's CLOSER — METAL, and it is **once-blocked now, not twice**
rmbp's `9d92f32c` added the `UNAOS_PRTSCRST` `K8_FEATS` arm, so the knob is reachable for a Pi
bare-metal image. Remaining half is pi's and unchanged: **fly with a card mounted** (`selftest_once()`
parks in its no-writable-volume arm on QEMU raspi4b), **then** add the REQUIRE line. Blocked only on
`9d92f32c` reaching this branch — i.e. on item 1.

## 8. CREATE `docs/dev/OS/pi-ledger.md` (LEDGER `S24`) — Peter deferred it to the focus turn
Lift pi's rows **verbatim**; leave the cross-arch rows on `docs/dev/LEDGER.md`. Note the gate hole pi 7
proved by mutation: `ledger-check.sh:105` carries `and path != "docs/dev/LEDGER.md"`, so the
over-arching ledger's OWN cross-refs are never resolved — **an arch ledger's refs ARE checked.**

## 9. PI-OWNED LEDGER ROWS with no arc yet
- **S19** — Pi serial has TWO producers (`pal.rs:2039`, `main.rs:3675`) and ONE drain (`main.rs:5500`).
  Belongs in the serial doc **with its command**; never inherit a producer count across boards.
- **S20** — `read_byte` returns `None` for both an all-ones LSR (open bus / wrong BASE) and no-data,
  swallowing the diagnostic a negative RX flight needs. Orin's copy got a raw-LSR witness (ORINRX);
  **the Pi copy is unchanged** (`arch/aarch64/serial.rs` ~:92).
- **S21** — Socket ABI: fourth instance of the unconditional-module / single-arch-consumer shape.
- **S28(c)** — `video/menubar.rs:77`'s `//!` line is the ONLY part of S28 pi owns (lane adjudicated by
  orin 15: the `display_tegra.rs` field keys are orin's, the `Cargo.toml` comments are rmbp's).

## 10. QUESTION OUTSTANDING TO ORIN since pi 7 — PRTSCR3's volume guard may be INERT ON THE PI
The guard checks presence AND publish-generation before every write. `USB_PUBLISH_GEN`
(`block.rs:757`) is bumped **only** by `publish_usb_geometry` (`:687`/`:705`); the Pi's SD card
registers via `block::register_sd` (`emmc2.rs:641`) and never advances it, and `emmc2.rs:177` says the
card's block count is *"published once by `install` and immutable after"*. **On the Pi the generation
is CONSTANT**, so the guard cannot distinguish same-disk from different-disk.
Two things to chase: **(a)** SR2/A36 must state the guard is USB-scoped, so nobody believes a Pi
capture survives a card swap; **(b)** confirm a constant generation cannot become a **PERMANENT
REFUSAL** — if the check wants an *advance* rather than a *match*, Pi Print Screen refuses every time,
on the board with no on-card log, on a path already recorded unflown.

## 11. TAIL — real, unscheduled
TABKEY metal half · flight-readiness close-the-window-under-load · U9/U10 · the four EL0 stack
sizings · R19's shut-out register for the V3D ladder · V3D `emptybin` Task 0.

---

## THE BENCH — read before any metal item above (7, 11)
- **`/dev/ttyACM0` is ONE PHYSICAL PROBE moved between boards BY HAND.** No process check tells you
  which board is on the far end. The butler is a content-routed SINGLETON that opens all four logs at
  startup, so a fresh `pi.log` mtime means a FILE OPEN, not Pi bytes.
- Liveness: `flatpak-spawn --host lsof -t /dev/ttyACM0`. Bare in-sandbox `lsof` says FREE while a host
  process holds it. **`find -newermt` is BROKEN here** (it is `bfs`, and `2>&1 | wc -l` counts error
  lines as matches) — use `stat -c %Y` / `ls -lt`.
- **THE PI HAS NO ON-CARD LOG. An unobserved Pi boot is evidence that never existed.**
- **Segment a capture by its `Loaded 'kernel8.img' … size 0x…` anchors before scoring anything.**

## THE METHOD THAT FOUND EVERYTHING — apply it to every item above
- **A check that cannot fire must SAY so rather than return green.** Five gate defects in one day,
  four in gates rmbp had just written; **every one found by MUTATING the tree or by reading at the
  other seat's sha — none by re-reading.** Being careful is not a method; it is what already failed.
- **To verify a gate, make it FAIL. To verify a branch, make it PRINT.**
- **Your own tree is authoritative for YOUR tree.** Right before ACCEPTING a peer's claim; NOT
  sufficient before CHALLENGING one — read at their sha (`git show <sha>:<path>`).
- **When the logic is decisive, stop counting.** `any(A,B) → any(A,B,C)` cannot change a build where
  A is true.
- **SP2/SP4 — a knob-off sha is a BASELINE GUARD ONLY.** Identical proves the baseline safe; it never
  proves a change works, and when the changed file is not in that build it is identical by
  construction. That guard is valid only because the knob-off image is HEAD-independent
  (`UNAOS_GIT_SHA` lives solely in `genet.rs:2572`; `genet` enters `K8_FEATS` only via `arroyo:5870`).
  **Re-verify that whenever `K8_FEATS` gains an arm.**

## APPENDED DURING SUPPORT ROUNDS
*(new items land here with the command that proves them, the turn they are found)*

### 2026-09-06 · orin 18's fold 8 — **THE `COUNT 26` COUPLING FIRES AT THE FOLD** (raises item 5 to blocking)
Found while adjudicating orin 18's grant ask. **Both incoming folds add `:: TSTE: … -> PASS ::`
emitters, and `pi4-regression.spec:262` is a GLOBAL census of that shape**, so the fold reds pi's
spec unless the COUNT is re-derived and bumped **in the same commit**.

- `dc683c40` (SHELLRELICS) adds **13 `verdict(...)` legs** — 2 in `shell_relics_witness`
  (`#[cfg(feature = "witness")]`), **11 in `shell_relics_native_witness`, gated
  `#[cfg(all(target_arch = "aarch64", feature = "baremetal", feature = "witness"))]` = exactly pi's
  `kernel8-test` config**; `shell_relics_witness()` is called from `midden_witness()`.
- `1aae3459` (VFSROUTE) adds **2 more** `:: TSTE: {} -> PASS ::` emitters (6 TSTE line additions).
  "No Pi-lane file" is TRUE and does not clear it — `shell.rs` is in the Pi image.
- `TSTE` is already scored here: `:2009-2012` REQUIRE four `midden.*` PASS lines, `:2013` FORBIDs
  `midden.\w+ -> FAIL`.

**Proof by execution — run the spec's own regex, do not reason about it:**

    python3 - <<'PY'
    import re,io
    spec=io.open('unaos/scripts/specs/pi4-regression.spec',encoding='utf-8',errors='replace').read().splitlines()
    rx=re.compile([l for l in spec if l.startswith('COUNT 26 ')][0][len('COUNT 26 '):])
    for t in [":: TSTE: shell.relics.renamed -> PASS ::", ":: TSTE: foo -> FAIL (got x) ::"]:
        print(("MATCH " if rx.search(t) else "no    ")+t)
    PY

**DO NOT PREDICT THE NEW NUMBER — MEASURE IT.** Several native legs may park on QEMU raspi4b for want
of a writable volume, and a parked/failing leg prints `FAIL`, which does **not** match the pattern.
⚠ **A COUNT over PASS-ONLY lines makes a flaky leg a COUNT error, not a leg failure** — the 13 native
legs put pi's census on volume state. Consider whether the rule should key on the leg, not the census.

⚠ **UNRESOLVED, flagged not asserted:** replaying that regex offline over `unaos/target/serial-pi.log`
(Sep 5) yields **30**, not 26 — 4 TSTE + 26 others, and the log carries **zero
`Loaded 'kernel8.img'` anchors** (a QEMU capture, unsegmentable by the usual anchor). Either that file
is not the artifact the harness scores, or the replay differs from the scorer's matching.
**Resolve this BEFORE quoting any post-fold target.** Scorer: `unaos/scripts/orin-specscore.py`
(`COUNT` is `failable` alongside REQUIRE/FORBID, `:582`/`:597`).

**Grant given the same turn:** `docs/dev/OS/06_NETWORK_STACK/pi_genet.md` 6/6 — verb-name prose only,
`::` witness TAGS (`fs5`, `ls1`, `ui3`) untouched. Also noted for the record: `dc683c40` **does** touch
`arch/x86_64/smp.rs` (1/1 comment, `usbinfo`→`lsusb`) — **the pi-8 baton said it did not, and that was
wrong**; and `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` 11/11 is a second aarch64-shared doc outside the
granted four (same rename class, Orin bench prose, left to orin).

**PRTSCR3 volume guard — item 10 CLOSED by orin 18, verified here at their sha**
(`git show 98ffd63d:…/video/prtscr.rs`, `volume_alive` :822): `if !self.usb_backed { return true; }`.
It wants a **MATCH**, so **(b) is dead — Pi Print Screen never refuses on this guard.** **(a) stands
with a corrected mechanism: inert on the Pi by EARLY RETURN, not by the constant generation** — the
generation never loads. Text is rmbp's file and row (SR2/A36); orin relays.

### 2026-09-06 · **RETRACTION — the entry above is WRONG in its direction, and so is baton item 5**
Resolved by reading the scorer instead of inheriting the baton's sentence. **Struck, not softened.**

**`COUNT <n> <regex>` is a FLOOR, not a census.** `unaos/scripts/mbench.py:21` — *"must match >= n
lines, else FAIL"*; `:184-185` `satisfied()` = `self.hits >= self.need`. **`mbench.py` is the gate**
that scores `pi4-regression.spec` (`unaos/arroyo:6325`). `orin-specscore.py` — whose `failable`/cover
language the entry above quotes — is a **coverage auditor, NOT the gate**; citing it was the error.
`mbench.py` has no cover / orphan / unaccounted-line check at all (grep: zero hits).

**Consequences, all three worth keeping:**
1. **ADDING witness lines can NEVER red a COUNT.** 26 → 39 still satisfies `>= 26`. Both incoming
   folds are clean; nothing needed bumping.
2. **BATON ITEM 5 IS FALSE AS WRITTEN.** *"any seat adding any `::` witness anywhere must bump it or
   pi's spec reds"* — no. **Do not pass it on.** What is TRUE is the mirror image: **a floor reds when
   matching lines are REMOVED or RENAMED AWAY**, which is exactly what a rename batch can do. That is
   the check to keep. Measured on both folds: `-> PASS ::` lines removed = **0** and **0**. Clean.
3. **The 30-vs-26 "discrepancy" was never one** — 30 hits against a floor of 26 is satisfied. It was
   an artifact of the wrong model, not a finding. **DUPGUARD's `COUNT 26→27` was therefore never
   forced**: raising a floor is a deliberate tightening, not a repair.

**THE LESSON, which cost two peer seats a round each:** the blocker went out because the baton said so
and the sentence sounded mechanical. **`verification-comes-from-execution` — read the scorer, run the
regex, make it fail.** Being careful is not a method; it is what already failed.

### 2026-09-06 · **ITEM 10 WAS NEVER OPEN — pi 7 had already answered it, in orin's row**
`98ffd63d:docs/dev/OS/orin-ledger.md`, A36, verbatim: *"scope note (pi 7): the mid-capture volume
guard `volume_alive()` is USB-scoped by `usb_backed()` … the guard is INERT for a Pi card capture BY
DESIGN (a card swap mid-capture on the Pi is unguarded; a constant publish generation can never become
a permanent refusal because the SD path is not evaluated)."* **Both halves (a) and (b), already
recorded.** Pi's baton and resume carried it forward as "asked, unanswered at close"; pi 8 re-asked it
and **two peer seats each spent a round re-answering a question the tree had already closed.**

**NORM — a baton's "open question" is a CLAIM, and it gets the same fresh check as a sha:**

    git show <peer-head>:<their ledger>      # before re-asking anything a baton calls outstanding

**AND: `video/prtscr.rs`'s header needs NOTHING** — `98ffd63d:178-186` already says *"A capture that is
not USB-backed (the Pi's microSD, QEMU's `test-fat` image) skips the probe entirely and costs
nothing"*, and `:611-613` documents the reason at `usb_backed` itself (*"pulling an unrelated stick
must not refuse it"*). ⚠ **Told both seats to leave it: `prtscr.rs` is UNCONDITIONAL and is the ONE
file in the fold that legitimately moves the knob-off image** — a no-op prose edit there moves pi's
baseline for nothing and burns the fold's cleanest control.

**NEW PI-BENCH QUESTION (rmbp 15, low priority, NOT convicted and NOT ruled out):** an SD hot-swap
mid-capture is **unguarded on the Pi** by the design above. Nobody has shown it reachable — the card
is not hot-pluggable in normal use. Needs a bench turn, and remember the Pi has no on-card log.

**rmbp 15's `171fee58`** (SR2 guard scope corrected on hw-rmbp) verified here: commit exists, touches
`docs/dev/LEDGER.md`, **unreachable from all four origin heads** — genuinely unpushed, push owed.

### 2026-09-06 · **QUARRY's cache invalidation is blind to SD volume change on the Pi** (orin 18; corrected here)
`video/quarry/live.rs:456-457` — `fn volume_gen() -> u64 { crate::drivers::block::usb_publish_gen() }`
(aarch64 arm), consumed `:754-757`: `if now_gen != self.cache_gen { self.cache.clear(); }`.
`USB_PUBLISH_GEN.fetch_add` has exactly two sites, `block.rs:694` and `:710`, both inside
`publish_usb_geometry`; **`register_sd` (`:827`) advances nothing.** Verified in this tree; orin's line
numbers differ by pi's 94-commit lag, same code. Window-open and the `r` key still invalidate, so the
cache is **stale-on-volume-change, not dead.**

⚠ **CORRECTION MADE TO ORIN'S FRAMING — do not inherit "on the Pi the generation is constant".**
`publish_usb_geometry` **IS reachable on the Pi**: `drivers/xhci/mod.rs:11883` publishes, `:6077` /
`:10885` / `:13893` retract; the Pi 4's USB is behind xHCI and `emmc2.rs:652-656` names the aarch64
`publish_usb_geometry` and the `/usb` mount. **A stick arriving or leaving on the Pi DOES advance the
generation and quarry's invalidation DOES fire.**

**The precise defect: quarry is blind to SD-CARD volume change specifically**, because `register_sd`
is the one publish path that never advances the generation — and **the Pi's card is the writable one**,
so a mid-session card change plus a cached listing is reachable here in a way it is not on the Orin
(read-only card). Narrower than "constant", still real, and it is the SP3 asymmetry in miniature: the
broad version tells the next reader a mechanism is ABSENT when it is present.

Owner: rmbp (`video/quarry`) — orin offered it as SO20 with owner rmbp or an rmbp B-row; pi has no
lane objection either way. Kept here so it survives whichever row it lands on.

### 2026-09-06 · VFSROUTE `same_volume` — **the Pi has the same two-filesystems-on-one-card shape**
rmbp 15's blocking condition on VFSROUTE: `MountTable::same_volume` compares constructor STRINGS
(`fs/vfs.rs:499-503`), while Tegra's `sdmmc_root_bind` mounts one card twice ("card" at `/`, "fat" at
`/fat`), so one physical volume reads as two. **`fs/vfs.rs` already exists in pi's tree (77 KB), so pi
inherits whatever lands.**

⚠ **Pi's instance to watch at the fold:** `emmc2.rs:688` mounts p1 as the FAT program volume via
`BlockSource::Default` while **unafs is sized from the same card** (`:188-191`), and `:654` describes
the `/fat` mount — one physical card, two filesystems. The replacement identity is said to derive from
**block source + FAT serial**; **pi's unafs volume has no FAT serial at all.** Make sure the derivation
answers for a non-FAT volume on the same physical device instead of collapsing it into the FAT one.

### 2026-09-06 · **LAYOUT (render9) vs pi's specs — ⚠ THE ENTRY BELOW WAS UNDER-SCOPED; SEE THE CORRECTION**
The LAYOUT patch riding render9 is a rename batch over witness-bearing text (`/fat` → `/boot`,
programs → `/apps`) — i.e. exactly the direction that CAN red a floor (see the retraction above).
Measured here at `059e04db`, so the answer is on the record before the patch exists:

- **Path exposure: NONE.** `/fat` appears twice in `pi4-regression.spec` and **both are comment
  lines** (`:2335`, `:2336`). All 8 scored directives that look path-bearing are `/`-as-separator
  false positives (`GPR/FP/TPIDR`, `owner/grants`, `240000/240000`, `5/5`, `rate=…/s`). **No REQUIRE,
  COUNT or FORBID names a filesystem path**, and none names a program load path.
- ⚠ **FOUR scored lines carry `FAT` in a WITNESS TAG:** `:442` `REQUIRE FATDIRS:.*delete_located\)
  PASS` · `:443` `FORBID FATDIRS:.*FAIL` · `:448` `REQUIRE FATMOVE:.*keep-chain\) PASS` · `:449`
  `FORBID FATMOVE:.*FAIL`. **Path-scoped sweep ⇒ untouched. Token-scoped `fat`→`boot` sweep ⇒ all four
  die.** The two REQUIREs red loudly (fine). **The two FORBIDs go DEAD SILENTLY and keep printing
  clean** — and they are the rules standing between this board and a silent FAT regression.

**THE GENERALISED CHECK — `grep -c '^-.*-> PASS ::'` over the diff is NOT sufficient**, because a
FORBID line contains no `-> PASS ::` and every dead FORBID slips through it. For each token a rename
batch touches:

    grep -n -iE '^(REQUIRE|COUNT|FORBID).*<token>' unaos/scripts/specs/pi4-regression.spec

**Treat a hit on a FORBID as BLOCKING, not informational** — a REQUIRE that stops matching reds; a
FORBID that stops matching reads green forever. Same family as [[a-check-that-cannot-fire]].

### 2026-09-06 · quarry item is now **SR3** on `docs/dev/LEDGER.md` (rmbp `f0670cd5`, UNPUSHED at writing)
rmbp 15 filed it in the narrow form pi corrected it to: a **stick** arrival invalidates quarry's
listing cache on the Pi; a **card** event never does. Owner rmbp (`video/quarry`), pi's board. No
action asked of pi — meet it at the next Pi bench turn. ⚠ The row is not reachable from any origin
head yet, so a grep for `SR3` in this tree finds nothing until the push + fold (P14/S31's
landing-lag-absence shape).

**PUSH STATE, verified 20:54Z (predicates, not values):** `origin/hw-rmbp` = `66c04fee`; rmbp's
`171fee58`, `511890bd`, `f0670cd5` are **NOT ancestors of it** — all three unpushed, one clean
fast-forward. ⚠ rmbp read origin moving `5c3dbb7e` → `66c04fee` as their own commits landing; it was
the earlier ones. **`ls-remote` answers "what is the value now", never "did MY commit get there" —
only ancestry does** ([[ls-remote-answers-value-reflog-answers-movement]]).

### 2026-09-06 · **`UNAOS_FBW`/`UNAOS_FBH` ARE TRACKED — and a build made with them must never be flashed**
An orin relics audit reported that `UNAOS_FBW`/`UNAOS_FBH` have no rebuild trigger (no `build.rs`).
**Refuted, and re-verified in THIS tree:** `env-dep:UNAOS_FBW` and `env-dep:UNAOS_FBH` are both present
in the aarch64 dep-info, so cargo invalidates on a geometry change.

    find unaos/target -name '*.d' -path '*aarch64*' -exec grep -h -o 'env-dep:UNAOS_[A-Z0-9_]*' {} \; | sort | uniq -c

(2 units each; `UNAOS_SMPPROBE`/`UNAOS_NOJB11`/`UNAOS_NET4*`/`UNAOS_DMAWIN` at 32; `UNAOS_GIT_SHA` at 1,
which corroborates **SP4**'s single-site finding.) **Rule kept: a missing `build.rs` proves NOTHING
about env tracking — ask the dep-info file.**

⚠ **DO NOT INHERIT THE TWO-LEGGED VERSION.** orin's refutation added *"the flight knobs become cargo
features anyway"* — **false for these two.** `arroyo:65-66`: *"UNAOS_FBW / UNAOS_FBH are a
PANEL-GEOMETRY OVERRIDE, **not a cargo feature** — they are read with `option_env!` by
arch/aarch64/mailbox.rs"*; `mailbox.rs:150-151` is `parse_u32(option_env!("UNAOS_FBW"))`. **The whole
refutation rests on dep-info for exactly the pair the audit named.** One leg, stated as one.

⚠⚠ **CARD-WRITE DISCIPLINE, PI-SPECIFIC — `arroyo:71-72`: "these change compiled-in behaviour: a build
made with them is NOT the flashable default image."** Combined with **the Pi having no on-card log**,
an `UNAOS_FBW=1920 UNAOS_FBH=1200` image flashed by mistake is **UNDETECTABLE FROM THE BOARD** — it
boots, it looks plausible, and nothing on the card records which geometry was compiled in. **Before any
Pi card write: prove the image was built knob-off**, and score the boot by the loaded image's identity
([[verify-what-booted-not-what-you-wrote]]), never by the write's sha alone.

### 2026-09-06 · VOLID re-cut — **the one case whose fingerprint side is structurally ABSENT is pi's**
orin stopped the volume-identity patch mid-cut: it derived identity from `fs::fat::volume_serials`
(`fat.rs:1759` here / their `:1742`), a **DEVICE-WIDE census** — two partitions on one device would
have received the same identity, reproducing the defect one layer down (rmbp 15 caught it).
Re-cut uses `FatFs::volume_fingerprint()` (`fat.rs:2094` here / their `:2077`, `(BS_VolID,
count_of_clusters)`) paired with `BlockSource`.

⚠ **`volume_fingerprint()` is a METHOD ON `FatFs`. Pi's unafs volume is not a `FatFs` and has no
fingerprint to pair.** The brief must answer *"what identity does a NON-FAT volume on a shared device
get?"* — **if it degrades to `BlockSource` alone, Pi `/` (unafs) and `/fat` (FAT, also
`BlockSource::Default`) collapse to one identity again**, which is `volume_serials`'s defect moved down
a layer. Asked orin to make **"unafs vs FAT on one Pi card ⇒ same_volume FALSE"** a MEASURED value in
the brief, not a derived expectation: it is the only case where the fingerprint side is absent rather
than merely different.

### 2026-09-06 · PUSH STATE CLOSED — **nothing owed to Peter, pi or rmbp**
`origin/hw-rmbp` = `13375781` at 22:56Z; rmbp's `171fee58`, `511890bd`, `f0670cd5`, `13375781` are
**all ancestors of it** — the whole set landed. ⚠ **I reported "owed" twice tonight; both were true
when said and false shortly after.** The fragile part was neither the value nor the provenance but the
ELAPSED TIME. **Re-run the fetch before reporting any push as outstanding** — a stale "owed" is a stale
sha wearing a fresher timestamp.

### 2026-09-06 · **CORRECTION — LAYOUT DOES RED PI. `pi4-barename.spec:63`, and the fix is NOT mechanical**
The LAYOUT entry above swept **`pi4-regression.spec` only** and was reported to orin as the board's
answer while they were gating their fold on it. **Pi has THREE spec artifacts.** Caught by rmbp 15.
**This is S31 pointed at this seat — a sweep bounds only the file you ran it in** — and it is the same
defect as the `COUNT` blocker in different clothes: a TRUE result relayed at a WIDER SCOPE than the
check behind it (rmbp's B61 shape, now with an instance from a second seat).

**THE COMPLETE SWEEP — run this one, not the old one:**

    for S in unaos/scripts/specs/pi4-regression.spec unaos/scripts/specs/pi4-barename.spec; do
      grep -n -E '^(REQUIRE|COUNT|FORBID)' $S | grep -E '/(fat|boot|apps|usb|fs)\b|/[A-Z0-9_]+\.(ELF|TXT|PNG|MD)'
    done

    pi4-regression.spec   118 REQUIRE + 2 COUNT + 134 FORBID  →  ZERO path-bearing
    pi4-barename.spec       3 REQUIRE + 0 COUNT +   3 FORBID  →  ONE, line 63

`:63` `REQUIRE :: BAREXEC: /fat/VUG\.ELF \(typed 'vug'\) — loaded … DETACHED, left RUNNING ::` — and
**it fires**: `arroyo:6216` runs `pi4-barename.spec` as the typed battery
(`k8_spec="${UNAOS_K8_SPEC:-…}"`). `/fat` is 4× across pi's spec files; the other three are comments
(`barename.spec:52`, `barename.inject:33`, `regression.spec:2335-2336`) and inert.

⚠ **THE FIX IS NOT A SUBSTITUTION.** LAYOUT does TWO things — `/fat` → `/boot` **and** programs →
`/apps`. `barename.spec:52` says `:63` tests *"probe 2 of `exec_resolve` (the **program-source
volume**)"*. So the corrected string is **`/boot/VUG.ELF` if the program source stays the boot volume,
`/apps/VUG.ELF` if programs move** — different specs of different behaviour. **A blind `/fat`→`/boot`
rewrite encodes the wrong answer.** Asked orin which volume probe 2 resolves to post-LAYOUT; write the
line only once that is answered.

⚠ **SECOND FLOOR, never previously recorded anywhere:** `pi4-barename.spec` = **3 REQUIRE + 0 COUNT =
3**, separate from `pi4-regression.spec`'s **120**. **Two specs, two floors, one board** — and one
broken REQUIRE is a third of the barename floor. The baton's floor guidance covered only the 120.

**STANDING COMMITMENT to orin, correctly scoped:** when LAYOUT's diff exists, run the token sweep over
**all three** pi spec artifacts, REQUIRE and FORBID, case-insensitive, **FORBID hit = blocking**.

### 2026-09-06 · `FATDIRS`/`FATMOVE` are LIVE coverage — verified before spending a gate on them
Both witnesses fire in pi's own capture, so the four directives at `pi4-regression.spec:442-449` are
real: the two REQUIREs are satisfied, the two FORBIDs are armed.

    awk 'index($0,"FATDIRS")||index($0,"FATMOVE")' unaos/target/serial-pi.log
    :: FATDIRS: fat.rs directory create/remove — create_dir(child .,..+publish), remove_dir(empty-only via delete_located) PASS …
    :: FATMOVE: fat.rs rename_entry(in-place 8.3 name RMW) + move_entry(dst-entry-first, then 0xE5 src keep-chain) PASS …

**Why this check mattered:** a gate asserting the survival of rules that never fire is theatre
([[a-check-that-cannot-fire]]). Establish that the protected rules are live BEFORE spending a gate on
protecting them. **LAYOUT's gate must carry BOTH shapes: four TAGS that must NOT move, and one PATH
(`barename.spec:63`) that MUST** — an "untouched" assertion across all five is wrong on the fifth.

**Also confirmed:** `grep -c -i 'fbw\|fbh' unaos/crates/kernel/Cargo.toml` = **0** — no cargo feature
for the panel knobs, so the dep-info leg is the only one holding.

### 2026-09-06 · `barename.spec:63` — the answer is **`/apps/VUG.ELF`**, and the line is DERIVED not pasted
orin 18's seat call, structurally verified here: Peter's ruling was TWO things — `/fat`→`/boot` renames
the BOOT VOLUME, `/apps` moves the PROGRAMS. Probe 2 of `exec_resolve` is the program-source root:

    shell.rs:5152  const EXEC_ROOT: &str = "/fat";        (orin's :6030 — the 94-commit lag)
    shell.rs:5164  /// 2. **The program-source root**, [`EXEC_ROOT`] — only for a RELATIVE token…
    shell.rs:5188  let from_root = normalize_path(EXEC_ROOT, name);

After LAYOUT `EXEC_ROOT` becomes `/apps`, so probe 2 resolves `vug` → `/apps/VUG.ELF`. `/boot` becomes
what its name says (loader, kernel.elf, MANIFEST, SRC.*) and stops being where programs live.
`barename.spec:52`'s prose must follow — "the program-source volume, `/apps`".

⚠ **DERIVE THE STRING, DO NOT PASTE IT.** The witness text comes from
`normalize_path(EXEC_ROOT, name)`, so at fold time **derive the expected value from the patch's actual
`EXEC_ROOT` through `normalize_path`** — never from the value a peer typed. A shipped
`EXEC_ROOT = "/apps/"` (trailing slash), or a different join, yields a witness neither seat wrote, and
a REQUIRE naming a nearly-right path reds on the typed run looking like a behaviour regression.

### 2026-09-06 · GATE-K8REACH (item 6) gains a worked row: **`UNAOS_DMAWIN` is NOT Pi-live**
Sites are `arch/aarch64/rtl8168_tegra.rs:3681` / `:4251` / `:4255` (ORIN-DMA-WINDOW, the Orin's Realtek
NIC) and `grep -c UNAOS_DMAWIN unaos/arroyo` = **0**. **Disposition: one of the 39 rows with no Pi-live
cfg site — arch-homed in a tegra file, unreachable from any Pi image, unmapped by any command.** Note
the shape for the rest of the pass: an **arch-NEUTRAL-looking name on an arch-HOMED site** is exactly
where the classifier's blind spots live (cf. `nvidia-kepler`, `rastmc`). Cite `--evidence`, never a
reason string.

### 2026-09-06 · **THE SHAPE OF THE WHOLE EVENING — all three seats, one defect**
A TRUE measurement relayed past the population it was taken over. Instances tonight:
- **pi**: swept `pi4-regression.spec`, reported "the board" (3 spec artifacts). Caught by rmbp.
- **pi**: carried the baton's `COUNT` sentence past the gate it described. Caught by reading `mbench.py`.
- **orin**: measured dep-info for FBW/FBH + features for the flight knobs, wrote one sentence covering
  both; and a shallow glob reporting 1 `option_env!` knob where a recursive grep finds **12**.
- **rmbp**: verified the CODE claim (`volume_alive`'s early return), relayed an UNREAD TEXT claim; and
  a push count right in value, stale in provenance.
**Two axes: SCOPE (wider population than the check) and TIME (later than the check).** rmbp's B61 names
the first; the second is its sibling. **Per-knob/per-file tables beat class sentences precisely because
the class kept being wrong.** Structural, not personal — worth a law in that form, not three
corrections.

### 2026-09-06 · **`NativeBackend` name-comparison — the volume-identity defect on PI'S SIDE of the seam**
orin's VOLID executor found a case beyond rmbp's C1: **two `NativeBackend`s constructed with different
names read as two volumes although there is exactly one native mount** — the same string-comparison
defect, on the NATIVE side rather than the FAT side. **This is pi's exposure directly: unafs IS the Pi's
native mount** (`emmc2.rs:188-191` sizes it from the card; `:703` `fs::unafs::locate()`). Same fix
covers it, per orin. **Check at the fold that the native side gets a real identity and not a name** —
this is the third layer the same defect has appeared at (device census → FAT serial → native name).

### 2026-09-06 · **A THIRD AXIS: OBSERVABILITY — "nothing owed" is not a peer-verifiable claim**
rmbp 15's correction, and it is sharper than the fetch rule. **An ancestry claim is MONOTONE** — once
`<sha>` is an ancestor of origin it stays one, so any seat can verify and relay it forever. **"Nothing
else is owed" is NON-MONOTONE and not peer-verifiable at all**: it is a statement about the peer's
WORKING BRANCH, which only that seat can see move. pi 8 verified four shas by ancestry (correctly) and
then answered a different question — **and told Peter to stop looking.**

**RULE: a peer can confirm that a commit LANDED; only the branch's own seat can say the list is EMPTY.**
Report `<sha> is an ancestor of origin/<branch> as of <time>` — never "nothing owed" for a branch this
seat does not own. (Postscript: `b990ebd7` was itself pushed by the time pi checked, 23:03:44Z, so even
rmbp's "one owed" was stale within minutes — which is the TIME axis stacked on the OBSERVABILITY one.)

### 2026-09-06 · **THE VOLUME-IDENTITY ARC — measured scope: FOUR minters, TWO arch arms, THREE boards**
orin 18 ruled VOLID ships as designed (fingerprint+source on the FAT side, distinct native identity) and
carried the bottom-layer redesign — **identity assigned at REGISTRATION, so every backend merely carries
it** — as its own arc. Right call: block layer is shared kernel core, rmbp's review is already spent,
and "correct but not final" is a legitimate place to ship from.

**Measured here at `059e04db` — the arc is bigger than the three paths named:**

    block.rs:687   pub fn publish_usb_geometry(dev)       ┐ TWO DEFINITIONS, cfg'd per arch
    block.rs:705   pub fn publish_usb_geometry(dev)       ┘
    block.rs:827   pub fn register_sd(dev)                  ← Pi microSD
    block.rs:1484  pub fn register_sdhc(num_blocks, …)      ← internal SD slot = RMBP'S BOARD
    block.rs:1681  pub fn register_tegra_sd(num_blocks, …)  ← Orin

**`register_sdhc` makes this a THREE-board arc** — rmbp is a participant, not a reviewer (their own SR2
correction names the internal Sdhc case). ⚠ **`publish_usb_geometry`'s two cfg'd arms are how this arc
fails quietly: an epoch minted in one arm only is an ARCH-ASYMMETRIC IDENTITY** — the FC-2 /
single-arch-consumer shape, already documented four times (`flight_recorder`, `dock`, `pidesk`, socket
ABI; S5/S21). It fails silently on whichever arch nobody flew.

**Precondition measured — no registration epoch exists today.** `PUBLISH_GEN` refs inside each body:
`register_sd` **0** · `register_sdhc` **0** · `register_tegra_sd` **0** · `publish_usb_geometry` **1** ·
`unpublish_usb_geometry` **2**. So quarry has nothing registration-minted to read, which is exactly why
SR3 takes the form it does.

**PROPOSED ACCEPTANCE CRITERION for the arc (sent to orin): the arc is DONE when quarry's `volume_gen()`
can read the registration epoch and SR3 closes without a second change.** Two findings, one root — the
same defect has now appeared at three depths (device census → FAT serial → native name), and a defect
that reappears each time it is pushed down a layer is telling you the property belongs at the bottom.

**THE THREE-QUESTION CHECK, with pi's amendment — ask (c) FIRST, it is cheapest and disqualifies
fastest:** (c) **observability** — could this command have seen the answer at all? · (a) **scope** —
what could it not have seen? · (b) **time** — has it decayed? pi 8's "nothing owed" survived two correct
checks and would have died at (c) in one step.

### 2026-09-06 · arc brief filed: [`volume-identity-arc.md`](~/.claude/plans/unaos/wip/volume-identity-arc.md) — **layer 3 is LATENT on pi**
orin 18 wrote the arc to a file so it outlives both seats; pi's four measured numbers and the
acceptance criterion are in it, and it is properly anchored (*"verified at `hw-jetson 98ffd63d`"*),
which is what makes its `fat.rs:2077` resolvable from this tree where the same function is `:2094`.

**Pi-side claims checked here:**
- **Layer 3's premise HOLDS** — `with_unafs` is one `pub fn` (`fs/unafs.rs:849`) with **82 callers**,
  no second entry point. Two differently-named `NativeBackend`s really would split one volume.
- ⚠ **But on hw-pi4 layer 3 is LATENT, not live: THREE `NativeBackend::new` sites and all three pass
  the same literal** — `shell.rs:5409` (the live mount), `fs/vfs.rs:1426`, `:1602`, all `"native"`
  (`fs/unafs.rs:618` is a doc comment; 4 grep hits, 3 real). **The string comparison returns the right
  answer BY ACCIDENT.**

**What that changes, and it is the actionable part: a Pi run today CANNOT exercise layer 3** — it
passes whether the fix is right or wrong ([[a-check-that-cannot-fire]]). **Pi's decisive case stays
layer 2**, the FAT fingerprint a native volume structurally cannot have. **Layer 3 needs a CONSTRUCTED
negative** — two `NativeBackend`s deliberately given different names over the one `with_unafs` volume —
or it ships with a green run behind it and no coverage. Told orin; it belongs in the five measured
values.

### 2026-09-06 · the two constructed negatives — **OPPOSITE DIRECTIONS, both fold-blocking on VOLID**
Checked the directions rather than nodding at them; both as orin states them are correct:

| case | construction | must answer | catches |
|---|---|---|---|
| rmbp's | two backends sharing ONE `volume_name` over **different** sources | **FALSE** | identity PRESENT BUT BYPASSED — a surviving name fallback answers TRUE |
| pi's | two `NativeBackend`s with **different** names over the ONE `with_unafs` volume | **TRUE** | layer 3, which no live configuration can reach |

**Neither is observable from a live mount table — that is why both must be CONSTRUCTED.** The two
remaining quadrants (same name/same source → TRUE, different name/different source → FALSE) pass today
by accident and prove nothing. Both cases are now fold-blocking on VOLID itself, not just on the future
arc: the patch shipping tonight needs them as much as the redesign does.

**House style adopted into the arc brief at pi's request:** every line number in a cross-seat brief
carries the tree and sha it was read in — that is what makes orin's `fat.rs:2077` resolvable from this
tree's `:2094`. Without it the brief would have carried a branch-assumption trap into the artifact
written to record that very lesson.

### 2026-09-07 · **LAYOUT SWEEP RUN — verdict: ONE DEFECT, `pi4-barename.inject:33`**
Patch `98beb60e` on `exec-orin18-fslayout` (5 commits on `cd91a3df`), 30 files, +850 −271. The sweep
this seat committed to was run against the real diff; **the predicted failure mode occurred**.

⛔ **`unaos/scripts/specs/pi4-barename.inject:33`** — the blind substitution:

    -# … Must resolve VUG.ELF on the program-source volume (`/fat`)
    +# … Must resolve VUG.ELF on the program-source volume (`/boot`)

**The program-source volume is `/apps`.** `EXEC_ROOT = "/apps"` (`shell.rs:6026` @ `98beb60e`), and the
patch's own `barename.spec:52` says `/apps` — **it contradicts itself across two files.** Comment only,
scores nothing, but it is SP3's expensive direction: it teaches the bare-name mechanism wrong in the
file that exists to explain the typed battery. Reported; one-line fix.

✅ **Everything else clean, verified at both shas rather than accepted:**
`grep -c '^-.*-> PASS ::'` = **0** · `pi4-regression.spec:442/443/448/449` **byte-identical** ·
emitters in pi-lane `arch/aarch64/syscall.rs` **FATDIRS 31→31, FATMOVE 58→58** ·
`regression.spec:2335-2336 → /boot` is **CORRECT** (mount table, not program source) · pi-lane numstat
matches (5 files: `display_tegra` 1/1, `mmu_tegra_el0` 1/1, `sched.rs` 3/3, `sdmmc_tegra` 40/18,
`syscall.rs` 50/22).

⚠ **`barename.spec:63` IS edited by the patch** (orin said it was not, and left it "to pi to write") —
**and it is written correctly**: `/apps/VUG\.ELF`. **Deriving before writing prevented a duplicate
edit.** The derivation: `EXEC_ROOT` literally `/apps`, probe 2 is `normalize_path(EXEC_ROOT, name)`,
BARENAME recovers the on-disk spelling from the parent listing.
**The trailing-slash risk pi flagged is STRUCTURALLY ABSENT** — `normalize_path` (`shell.rs:61-79`)
discards empty components, so `"/apps/"` normalises identically. Say so rather than banking a warning
that could not have fired.

**S35 (EL0 blobs stay in the volume root): NO OBJECTION.** `sys_open` takes an 8.3 leaf — an EL0 ABI
constraint; `find_app`-then-`find_in_root` is right. ⚠ **pi's gate cannot see this either way**: pi's
spec names `MIDDEN.BIN` in three comments only (`:79`, `:86`, `:481`), no scored directive names a blob
path — **so the gate would not have caught a wrong choice here.** Worth a REQUIRE at a future focus turn.

**Fold order accepted: fold 8 → VOLID → LAYOUT**, with `layout.volid` an expected Orin red in between
(flagged by orin, not discovered).

### 2026-09-07 · LAYOUT **CLEAR** — fix verified, and pi's scored-spec scope settled AUTHORITATIVELY
`99448d25` (1 file, 1/1) fixes `barename.inject:33` → `/apps`. Re-swept the fixed tip: **zero `/fat`
left in pi's artifacts**; `barename.spec:52`/`:63` = `/apps`, `regression.spec:2282-2283` = `/boot`
(mount table — correct token). Nothing outstanding on LAYOUT from pi.

⚠ **THE SCOPE QUESTION IS NOW ANSWERED BY THE HARNESS, NOT BY A FILENAME GLOB.** This seat had been
asserting "pi's three spec artifacts" from `ls unaos/scripts/specs/ | grep '^pi4'` — the same shape as
the sweep that missed `barename.spec` in the first place. **The authoritative check:**

    grep -n -- '--spec' unaos/arroyo
      6326:  --spec ".../scripts/specs/pi4-regression.spec" --platform pi
      6349:  --spec "$k8_spec" --platform pi        # default .../pi4-barename.spec

**Exactly TWO scored specs, both in the `kernel8-test` path. `test-arm` scores NONE.** Caveat: the
`UNAOS_K8_SPEC` env var can repoint `$k8_spec` at any spec, so two is the DEFAULT configuration, not a
structural bound. **Ask the harness what it scores; never infer coverage from a directory listing.**

**Fold order stands: fold 8 → VOLID → LAYOUT.** The inject fix re-gates on gate 9's folded tip
(`367106ef`) rather than a stale one. If the re-gate moves either scored spec, sweep again.

### 2026-09-07 · **COVERAGE HOLE: the two-mount rename is LIVE on the Pi and pi's gate cannot see it**
orin 19 asked whether `mv` across `/` (UnaFS) and the FAT program volume is exercised on pi. **It is
not.** No scored directive in either scored spec names `mv`; the only `mv`-adjacent text is
`pi4-regression.spec:446`, a comment on FATMOVE — *"move a file across dirs by reference"* — which is
**within-FAT**, not native↔FAT.

**Worse than shape-only: the path is REACHABLE on the Pi** (UnaFS at `/` and the FAT volume at `/fat`
come off the same physical card) **and unguarded.** A green Pi run is not evidence for any change to
`MountTable::rename`. **OWED: a REQUIRE for a native↔FAT `mv` at a focus turn.**

### 2026-09-07 · orin 19's ACL patch — **not derivable here; premise `/boot` is ahead of the tree**
`~/unaos-bench/scratch/orin19/aclsym2.patch` (base `777c31b0`) **does not apply to hw-pi4** —
`git apply --check` exit **1**, `fs/vfs.rs:624`. Ground absent here, checked at THEIR sha first:
`same_storage` **11 hits @777c31b0 / 0 here**, `volume_id` **9 / 0**. Landing-lag, not missing symbols.
**Running `kernel8-test` here would score an unpatched tree — a check that cannot fire.** Answer given
as *not derivable*, explicitly neither green nor red.

⚠ **`/boot` DOES NOT EXIST ON ANY PUSHED BRANCH.** hw-pi4 mounts the FAT volume at `/fat`
(`shell.rs:5410`, `EXEC_ROOT = "/fat"`, `RESERVED_VOLUME_PREFIXES = ["/usb","/fat"]`). LAYOUT is on
`exec-orin18-fslayout`, unpushed; fold order fold 8 → VOLID → LAYOUT. **Watch for `/boot` becoming a
fact by citation before it is a fact in the tree.**

**UnaFS root write-authz, answered from this tree** (`fs/vfs.rs:879` `native_write_authz`): kernel
principal → Ok · **no `owner` attribute → Ok, "public object, writable"** · owner match → Ok ·
`grants:<principal>` with `RIGHT_WRITE` → Ok · else `Denied`. **An owner-less root permits the shell
principal.** ⚠ **The real hazard is the first arm:** `read_inode(id)` failing → `Denied`, so a
`""`→root-inode resolution miss **fails CLOSED and is indistinguishable from an ACL refusal**. Mount
table normalises `""`→`/` (`:153`, `:308`); whether the native backend does is untested here.

### 2026-09-07 · METHOD — **a pipeline can swallow the exit code and manufacture a green**
`git apply --check <p> 2>&1 | head -5 && echo "APPLIES CLEAN"` printed **APPLIES CLEAN on a patch that
does not apply**: `&&` binds to `head`'s status, not `git apply`'s. Nearly relayed as a pass. **Read the
exit code on its own line (`cmd; echo $?`) whenever a command's VERDICT is the deliverable** — a
wrapper that swallows status is [[a-check-that-cannot-fire]] wearing a shell prompt.

### 2026-09-07 · **GRANT GIVEN to orin 19: the cross-volume `mv` leg — THREE PIECES, ORDERED**
Closes the coverage hole above. The gap is confirmed, not inferred: `dc683c40`'s existing leg is
native→native (`fs_mv(c, B, C, false)`, both operands at the native root). **No leg crosses backends.**

1. **Fixture leg** in `shell_relics_native_witness` (`#[cfg(all(aarch64, baremetal, witness))]` — already
   pi's `kernel8-test` config, no new gate): create `XVOL.TXT` at the native root with known bytes;
   `fs_mv` to the FAT program volume; assert **gone from `/`, present on FAT, bytes identical**; move
   back and assert the reverse.
   ⚠ **DERIVE THE DESTINATION PREFIX FROM THE MOUNT CONSTANT, NEVER A LITERAL** — `/fat` today, `/boot`
   after LAYOUT. A hardcoded `/fat` becomes a silent within-FAT no-op the day the mount moves: the very
   defect the leg exists to catch, hiding inside the test for it.
2. **Witness**: `verdict("shell.relics.mv.xvol", …)` → `:: TSTE: shell.relics.mv.xvol -> PASS ::`.
3. **Then** `REQUIRE :: TSTE: shell\.relics\.mv\.xvol -> PASS ::` (FORBID `-> FAIL` pairing optional,
   per the `midden` precedent at `:2013`).

⛔ **CONDITION: run the leg BEFORE writing the directive.** If native↔FAT `mv` is unimplemented or not
copy-then-delete, the leg fails and **that is a real finding, not a spec bug** — writing the REQUIRE
first manufactures a red and invites someone to "fix" the spec to match broken behaviour. Same rule that
keeps the `[wc-d] moved=` rule off this spec (item 3).

**Floor: 120 → 121** (118 REQUIRE + 2 COUNT today). **`COUNT 26` unaffected** — a floor only gains slack
from an added PASS line.

**Lane: granted to orin 19, scoped to those three pieces only.** Any other edit to `pi4-regression.spec`
comes back here first.

### 2026-09-07 · **`native_write_authz` fails closed BEFORE the kernel short-circuit — worth its own row**
Confirmed at this tree's line numbers: `:879` fn · `:885` `read_inode` · `:887` `Err(_) => Denied` ·
**`:889` `if principal == KERNEL_PRINCIPAL`**. **So rmbp's B68 result — "every caller passes
KERNEL_PRINCIPAL, so the ACL is dormant" — does NOT make the destination call inert.** The kernel
principal is subject to the resolution arm like any other, and a `""`→root-inode miss returns `Denied`
indistinguishably from an ACL refusal.

**GENERAL FORM, worth a row of its own: a fail-closed guard placed BEFORE a privilege short-circuit is
not dormant, whatever the callers pass.** Sibling of [[a-check-that-cannot-fire]] — here the check fires
when nobody expects it to, rather than never.

### 2026-09-07 · **THE `xvol` LEG WENT RED, THE CONDITION WAS RIGHT — and native↔FAT `mv` MUST refuse**
Observing before writing the directive (the condition pi attached to the grant) paid immediately:
native↔FAT `mv` **is refused by design** on orin's tree — `fs_mv` via `same_volume`, and
`MountTable::rename` via `same_storage` → `Unsupported`, with **no copy-then-delete fallback**. So the
leg was rewritten to assert the REFUSAL, and **that is strictly better than what pi granted**: a
negative control that reds on the VOLID-C1 aliasing regression (make `volume_id` medium-derived, the
two filesystems on one card compare equal, the move is admitted, every fact flips). Four sub-assertions
taken off the MOUNT TABLE, not the console — a console-only leg passes on a verb that refuses *after*
destroying the source. `two_volumes` asserted, never assumed, or the leg is vacuous. Mutation-proved.
**Grant confirmed for the new semantics.**

### 2026-09-07 · ⚠ **CORRECTION TO THIS SEAT'S OWN ANSWER — the path is NOT live on hw-pi4 today**
pi 8 told orin 19 *"the two-mount rename is LIVE on the Pi and pi's gate would not catch a regression."*
**The coverage half was right; the reachability half was relayed from orin's message unchecked.**
Measured at `059e04db`:

    grep -rn 'fn rename' --include='*.rs' unaos/crates/kernel/src/
      → fat.rs:3499 pub fn rename_entry(   ← the ONLY one. NO MountTable::rename on hw-pi4.
    fs_mv (shell.rs:1229) opens ONE volume via mount_write_volume and resolves BOTH operands in it.
    `same_volume` / "cross-volume" in shell.rs: 0 hits.

**A cross-volume `mv` on hw-pi4 is not unguarded — it is INEXPRESSIBLE.** No live exposure today; it
arrives with VFSROUTE at the fold. **Consequence for sequencing: the leg must be in the tree BEFORE pi
folds VFSROUTE**, so the guard lands in the same fold as the thing it guards.

### 2026-09-07 · **CORRECTION: only ONE spec is scored on a plain `kernel8-test`**
This seat told orin 18 "two scored specs, both in the `kernel8-test` path." More precisely, and this is
the version to keep: **`pi4-regression.spec` is scored on EVERY `kernel8-test`; `pi4-barename.spec` is
scored only in TYPED mode** (`$k8_spec`, armed by `UNAOS_K8_SCRIPT`). One spec on an unarmed run.

### 2026-09-07 · **A FORBID CANNOT CATCH A WITNESS THAT NEVER RAN — the family-level coverage hole**
`shell.relics.*`, `layout.*` and `vfsroute.*` have **no named directive in any spec in the tree**; the
only `TSTE` directives here are `midden.*` (`:2009-2013`) and `fatverb.*` (x86-fat). Those families are
guarded solely by mbench's builtin `DEFAULT_FORBIDS` (`mbench.py:135`) — which catches a leg that FAILS
and **cannot catch a family that stops running at all** (cfg drift, an early return, a fold dropping the
call site). The run still prints green. Composes with rmbp's four silent skip paths in `layout.mv`.

**FIX IS REQUIREs, NOT MORE FORBIDs — only a REQUIRE catches absence.** Granted to orin 19: `mv.xvol`
plus **one liveness-anchor REQUIRE per family** (`shell.relics.*`, `layout.*`, `vfsroute.*`) on the
`midden` template — four directives, floor 120 → 124 on this tree. **OWED AT A FOCUS TURN: naming every
leg individually** — that is where the floor arithmetic gets easy to get wrong, and each one needs the
same observe-then-write discipline that just saved `xvol`.

### 2026-09-07 · **GRANT EXTENDED to SEVEN directives in `pi4-regression.spec`** (orin 19, asked before folding)
`mv.xvol` + 3 liveness anchors + **`vfs.aclsym`, `vfs.aclsym.dir`, one paired FORBID**. Floor on THIS
tree: `118 + 4 + 2 = 124 REQUIRE + 2 COUNT = **126**` (orin's tree reads 125 — two right numbers,
neither quotable across trees). Beyond those seven, back to this seat.

**What justified the overrun — the mutation, not the intent: with `a62188c9`'s two `authorize_write`
lines REMOVED, `kernel8-test` still reported `MBENCH PASS 119/119`.** Every witness stayed green with
the fix gone. **THE SECOND EDGE, adopted: only a REQUIRE catches absence — AND a green COUNT can also
mean nothing was ever asked.** Different holes; the second is worse, because the number goes UP while
coverage goes to zero.

⛔ **CONDITION ON `vfs.aclsym`** — it is REQUIREd to have SPOKEN (`PASS` *or* a stated skip), not to have
passed, so an honest no-volume board is not reddened by a row about the ACL. Right shape. **But if it
always SKIPS on the pi4 gate config, on this board the row is bookkeeping that READS AS COVERAGE** —
then it must be labelled in the spec comment as a liveness anchor, not as ACL verification, and the skip
token must be **distinct and greppable** so a permanent skip is discoverable by search. Confirm which arm
it takes; do not assume.

### 2026-09-07 · **PI'S SPECS ARE DELIBERATELY foreman-CLEAN — and that reframes rmbp's x86 red**
`pi4-regression.spec` has 3 look-around occurrences, **all COMMENTS about the rule**: `:46` excludes
`(?=…) (?!…) (?<=…) (?<!…)` and backreferences **BY DESIGN**, `:62` documents the trailing
`(?!LITERAL)` convention that replaced them, `:628` states the consequence — *"a single `(?!…)` here
would make that evaluator reject the WHOLE spec and check nothing."* `pi4-barename.spec`: **zero**.
**Keep it that way; any new directive must be look-around-free.** `tools/foreman` is present in this tree,
so pi's side can be re-verified locally at any time.

⚠ **rmbp's row is bigger than "a test reds":** `x86-fat.spec:238` `FORBID :: RING-3 FAULT: task
'(?!u1b-)[^']*' KILLED` means foreman **rejects that whole spec and checks NOTHING**, silently, since
`c7326929` (2026-08-22). Relayed to orin 19 in that form for rmbp — "the test is red" and "the spec
checks nothing on this engine" triage completely differently, and pi's own `:628` is the sentence that
proves the distinction.

### 2026-09-07 · ⚠⚠ **THE PORTABILITY RULE IS DOCUMENTED ONLY ON hw-pi4 — that is why `x86-fat.spec:238` happened**
A citation dispute resolved into the round's best structural finding. orin 19 grepped all eleven specs
at `777c31b0` for the sentence pi quoted and got **0 hits**; pi's quote is verbatim real at
`059e04db`. **Both greps correct — S31's third direction, on a citation instead of a symbol.**

`pi4-regression.spec:626-629`, THIS tree:

    626 # ---    NOTE the FORBID is written look-around-free per the PORTABILITY RULE at the head of this
    627 # ---    file: `foreman` (Rust regex) refuses look-around and its preflight is all-or-nothing, so a
    628 # ---    single `(?!…)` here would make that evaluator reject the WHOLE spec and check nothing.
    629 # ---    Chain below is the documented prefix-factored form of "not followed by 1.00x".
    630 FORBID \[wc-b\] rollup .* amp=(?:$|[^1]|1(?:$|[^.]|\.(?:$|[^0]|0(?:$|[^0]|0(?:$|[^x])))))

**It is attached to the CHROMEBAND FORBID — pi's one extra REQUIRE, the 118-vs-117 difference — which
lives in pi's 36 UNLANDED commits. So does its documentation.** The rule that forbids look-around is
**invisible to both seats most likely to violate it**; rmbp wrote `(?!u1b-)` on 2026-08-22 into a spec
whose governing rule has never been reachable from trunk or hw-jetson. **Not carelessness — a rule
enforced by a comment in a file only one seat can read.**

**OWED AT THE FOCUS TURN (the second is the real one):**
1. pi's portability block lands with the fold — it rides CHROMEBAND, so it comes free.
2. **MOVE THE RULE somewhere all three seats read — `LAWS.md` or `STRUCTURAL_GATES.md` — not a comment
   at the head of one arch's spec.** Source text: `pi4-regression.spec:626-629` @ `hw-pi4 059e04db`.

**⛔ THE "GREEN BY VACUUM" CLAIM BELOW IS RETRACTED — SEE THE RETRACTION ENTRY. Kept, struck, not deleted:**
`tools/foreman/src/verdict.rs:269-275` — user rules are pushed at `:272` with `?`, and the builtin
`DEFAULT_FORBIDS` are installed at `:274-275` **in the same loop, AFTER**. So one bad pattern aborts
the parse **before the panic/FAIL safety net is installed**: the spec loses its own rules AND the
builtins. **"Green by vacuum."** A number that goes up while coverage goes to zero — the round's
recurring shape in its purest form.

**Census (orin, all eleven specs): exactly ONE non-comment look-around in the tree — `x86-fat.spec:238`,
rmbp's.** Pi's specs foreman-clean, now measured tree-wide rather than asserted per file.

### 2026-09-07 · `vfs.aclsym` condition CLOSED — PASS arm, not skip
Measured off the capture: `:: TSTE: vfs.aclsym -> PASS ::` and `vfs.aclsym.dir -> PASS ::` on the pi4
gate config. **Real ACL coverage on this board, not bookkeeping.** Skip token `vfs.aclsym skipped` is
distinct and greppable; the REQUIRE is look-around-free (alternation, not look-ahead); gate 17 prints
`aclsym-arm=` and `skips=` into the RESULT file, so a future flip to the skip arm is visible without
reading a capture. **A condition that must be re-checked by hand is a condition that stops being
checked** — worth carrying as a norm.

### 2026-09-07 · ⛔ **RETRACTION — "green by vacuum" is FALSE. foreman refuses LOUDLY.**
Verified in THIS tree, not taken on orin's word:

    tools/foreman/src/main.rs:114  verdict::preflight_spec(&cli.spec)...?;   ← runs FIRST
    tools/foreman/src/main.rs:115  let directives = verdict::parse_spec(...)?;
    tools/foreman/tests/preflight.rs present
    457ed7c5 (SPECFLIGHT): ancestor of hw-pi4 ✓ AND hw-jetson ✓   ← no per-tree divergence here

The `?` at `verdict.rs:272` does abort before `DEFAULT_FORBIDS` at `:274` — **but that path is
unreachable through the binary.** foreman stops before evaluation and NAMES the offending lines; no
verdict table prints. **The red is the instrument WORKING.** And `mbench.py` (Python `re`) accepts
look-around, so `x86-fat.spec`'s coverage is live in the gate `arroyo` actually uses. What is lost is
foreman's second opinion on one spec — real, worth fixing, **not a spec checking nothing.**

**THE PRECISE ERROR WAS THIS SEAT'S, and it is one word.** `:628`'s comment is CORRECT in every clause
(it even names the preflight as the mechanism). orin's `:269-275` read was correct in isolation.
**pi relayed it as "voids that spec's ENTIRE foreman coverage, SILENTLY" — "silently" appears in no
source.** That adverb is the entire distance between "an instrument refusing loudly" and "green by
vacuum", and it was manufactured in the relay, then handed back to orin with pi's endorsement on it.

**NEW RULE, the one this cost: "SOURCED FROM CODE" IS NOT "VERIFIED END-TO-END."** Reading a function is
a citation too. **The unit to verify is the path from ENTRY POINT to behaviour, not the function holding
the interesting line.** The observability question asked in the wrong place: pi asked *"is this
sourced?"* instead of *"could this path have been reached?"* — `grep -n 'parse_spec' main.rs` would have
settled it and neither seat ran it.

⚠ **RECORD IT AS THE COUNTER-EXAMPLE, NOT THE FIFTH INSTANCE** (rmbp's framing, correct): an instrument
that could have failed silently, was deliberately built not to (`457ed7c5`), and named exactly where to
look. **Four instances plus one defended near-miss teaches "build the preflight"; five failures teaches
"we cannot be trusted."** A fleet that only collects failures concludes the wrong thing about its own
instruments.

### 2026-09-07 · **EIGHTH DIRECTIVE GRANTED** — FORBID the `vfs.aclsym` skip line where the volume exists
rmbp's ask, and they are right that seven was one short. **It is pi's own condition made MECHANICAL
instead of manual** — the leg going quiet on the very board that has the volume was the hole pi named
and then left guarded by a promise. **REQUIRE catches the family disappearing; FORBID catches it taking
the wrong arm. Neither substitutes.**

On record so nobody softens it: the volume is structurally present on pi's gate config (measured
`-> PASS`; `xvol`'s `:: ls1: /: … apps/ boot/ ::` corroborates), and `pi4-regression.spec` scores only
pi's config, so the FORBID is sound here. ⚠ **If it ever reds, that is NOT a spec bug — it means the ACL
leg stopped testing on a board that can test it.** Do not let a future seat relax the rule; same failure
mode as writing a REQUIRE before observing.

**Grant now: EIGHT directives.** Floor `118 + 4 + 2 = 124 REQUIRE + 2 COUNT = 126`, FORBID +1.

### 2026-09-07 · **THE EIGHT DIRECTIVES ARRIVE UNRUN ON THIS BOARD — provenance table for the fold**
Gate 18 green on orin's tree: `MBENCH PASS 125/125 forbid=0`, `aclsym-arm=PASS`, `aclsym-skips=0`,
`xvol=1` (tip `06ffdaf8`). ⚠ **Every directive was written, measured and mutation-proved on hw-jetson.
hw-pi4 has executed NONE of them and cannot** — `xvol` needs `same_volume` / `MountTable::rename`, absent
here. **At pi's fold the code and its rules land together, and pi's fold gate is their FIRST execution on
this board.** Expect to read eight unfamiliar reds cold; that is normal, not a defect.

**WHAT EACH GUARDS — keep this; a red is only as diagnosable as the reason someone wrote the rule:**

| directive | guards | proved red by |
|---|---|---|
| `vfs.aclsym` (spoke-or-skip) | the ACL fix being deleted | removing `a62188c9`'s two `authorize_write` lines left `PASS 119/119` — **green with the fix gone** |
| `vfs.aclsym` skip-FORBID (8th) | the leg going quiet on a board that HAS the volume | rmbp's ask; `aclsym-skips=0` is the measured baseline |
| `vfs.aclsym.dir` (flat PASS) | the directory arm | mutation `refused=Err(NoSuchPath) want=Err(Denied)`, `control_ok=true` |
| `shell.relics.mv.xvol` | native↔FAT `mv` being ADMITTED | VOLID-C1 aliasing: medium-derived `volume_id` ⇒ two filesystems on one card compare equal ⇒ move admitted, every fact flips |
| 3 × liveness anchors (`shell.relics.*`, `layout.*`, `vfsroute.*`) | a witness FAMILY that stops running | `DEFAULT_FORBIDS` catches a FAIL, never an absence |

**Asked orin to put provenance in the SPEC COMMENT above each block, not in a landing report** — the
report will not be open when the red is. ⚠ **S16's inverse:** S16 is 106 invariants stated in comments
and enforced by nothing; this is the mirror — rules enforced by a gate and explained nowhere.
**The explanations are cheap now and archaeology in a month.**

**The row that matters most if only one gets explained: `vfs.aclsym`** — a gate that printed green while
the thing it existed to protect had been deleted.

**Floor: orin's tree 119 → 125. This tree lands at `118 + 4 + 2 = 124 REQUIRE + 2 COUNT = 126`, FORBID
+1.** Two right numbers, neither quotable across trees.

### 2026-09-07 · B61's FOURTH AXIS — **severity** (credited to this seat by rmbp 15)
- **scope** — a wider population than the check was taken over
- **time** — a later moment than the check was taken at
- **observability** — another tree than the check was run in
- **severity** — **an intensifier the check never licensed**

rmbp's reason, kept because it is better than pi's: *an adverb reads as part of a finding rather than as
a claim of its own, so nobody stops to ask what measured it.* ⚠ **It is not a fourth flavour of the same
mistake:** the first three describe a claim OUTRUNNING its check; severity describes a claim that never
had one **and did not look like it needed one.** That is why "silently" passed three seats with the code
open in front of one of them.

### 2026-09-07 · **PI IS THE REFERENCE for derived-vs-asserted mount census — keep it that way**
orin 19 found `sdmmc_tegra.rs:3495` printing *"unafs has no volume here"* as a **hardcoded string
literal** in the census `serial_println!`, with `sdmmc_root_bind` aliasing `/` onto FAT on that premise —
while a real UnaFS volume sat at slot 2, type `0x7f`, magic ok. Nothing tested the literal. They asked pi
to grep for the same shape.

**Result on hw-pi4: NOTHING of the shape.** `grep -rn 'has no volume\|no volume here\|were dead\|has no
device'` → 2 hits, both `main.rs` comments on unrelated matters, neither beside a mount decision.

**Because pi MEASURES it — `drivers/emmc2.rs:702-706`:**

    // p2 — the native volume. `locate` walks the same card's partition table by superblock magic.
    let native = match crate::fs::unafs::locate() {
        Ok(span)  => format!("base_lba={} blocks={}", span.base_lba, span.block_count),
        Err(e)    => format!("absent ({:?})", e),
    };

**`absent` is a RETURNED ERROR VALUE, not a literal** — the census cannot claim absence unless `locate()`
walked the table and failed.

⭐ **And the property worth protecting, `emmc2.rs:708-710`:** *"`absent` here is a statement about the
boot SEQUENCE (nothing USB was needed to get this far), not a claim that none will ever appear."*
**Pi's census already distinguishes "not found YET" from "does not EXIST", in writing, at the site** —
exactly the conflation that let the Orin alias `/` onto FAT. **`emmc2.rs:702-714` is the function to
port; do not let a future refactor collapse either property (derive the fact; say which kind of absence).**

### 2026-09-07 · orin 19 CLOSED — eight directives landed, sweep clean
`06ffdaf8..42eb2736`: 39 lines into `pi4-regression.spec`, **exactly ONE scored directive**
(`+FORBID aclsym: .*vfs\.aclsym skipped`), the other 38 provenance comments; `LAWS.md` +30; no other
file. **Grant honoured exactly — no overrun.** Gate 19 and 20 green, `125/125 forbid=0`,
`aclsym-arm=PASS skips=0`, FORBID 134 → 135 on their tree.

**Standing grant for orin 20 and after: EIGHT directives, all landed. Anything further comes back to
this seat, under the same condition — OBSERVE THE LEG BEFORE WRITING THE RULE.**

**Push predicate (monotone half only; the count is orin's to state):** `origin/hw-jetson = 06ffdaf8` at
close; `06ffdaf8` **ancestor/landed**, `42eb2736` **not an ancestor/owed**.

**NOT LANDING, kept as record:** `exec-orin19-bootns` `561c0436` binds `/` to a `ROOT/` subdir on the FAT
card — a workaround on the false premise. orin 20 rebinds `/` to the volume already mounted. Constraints
that stand: **2048 blocks = 1 MiB, read-only.**

---

# ⭐ READ THIS BEFORE THE APPEND LOG — STATE OF THE QUEUE, 2026-09-07 13:29Z
The header's 11-item list is the ORIGINAL ranking and is now stale in places. This block re-ranks it
against everything the append log recorded. **The append log stays authoritative for detail; this is the
index.** pi: `hw-pi4 059e04db`, clean, 0 unpushed.

| # | item | state now |
|---|---|---|
| 1 | **THE TRUNK FOLD (94 commits)** | **STILL #1, and it grew.** Now also carries VOLID, LAYOUT, the ACL fix and **eight new directives in pi's own spec that this board has never executed** (provenance table in the log). |
| 2 | Registry-full FORBID | unchanged, ready, one line |
| 3 | `[wc-d] moved=` rule | **still BLOCKED ON PURPOSE** — the discipline that later saved `xvol` |
| 4 | SP3 `emmc2.rs` header fix | unchanged, one line |
| 5 | `COUNT 26 ::` coupling | ⚠ **THE ORIGINAL ENTRY IS FALSE — see the retraction.** COUNT is a FLOOR (`hits >= n`). Adding never reds; **removing or renaming away** does. |
| 6 | GATE-K8REACH 104 rows | unchanged; **one row worked** (`UNAOS_DMAWIN` = not Pi-live, arch-homed, unmapped) |
| 7 | S14's closer | unchanged, metal, once-blocked |
| 8 | **create `docs/dev/OS/pi-ledger.md`** | unchanged — and this queue file is its seed |
| 9 | pi-owned rows S19/S20/S21/S28(c) | unchanged |
| 10 | PRTSCR3 volume-guard question | ✅ **CLOSED** — was never open; pi 7 had answered it in orin's A36 |
| 11 | tail (TABKEY, U9/U10, EL0 stacks, V3D…) | unchanged |

**NEW, RANKED, from this round:**

| rank | item | why |
|---|---|---|
| **A** | **Move the PORTABILITY RULE out of `pi4-regression.spec:626-629`** into LAWS/STRUCTURAL_GATES | a rule with no reachable home; it rides CHROMEBAND in pi's 36 unlanded commits. orin 19 took the LAWS half — **verify it landed, then keep the spec half** |
| **B** | **Guard `emmc2.rs:702-714`'s two properties** (absence is a returned value; "not found YET" ≠ "does not EXIST") | pi is the REFERENCE implementation; a refactor that collapses either re-creates the Orin's root-aliasing bug |
| **C** | Native↔FAT `mv` REQUIRE | ✅ **DONE by orin 19** as `shell.relics.mv.xvol` — asserts the REFUSAL. Arrives at the fold |
| **D** | Per-leg REQUIREs for `shell.relics.*` / `layout.*` / `vfsroute.*` | only liveness ANCHORS landed; per-leg naming is a focus-turn job, observe-then-write each |
| **E** | A REQUIRE naming an EL0 blob path (S35) | pi's gate cannot see where the blobs live either way |
| **F** | Pi SD hot-swap mid-capture (unguarded, unreached) | bench question; no on-card log |
| **G** | quarry SR3 (card events never invalidate) | rmbp's row, pi's board; meet it at the fold |

**STANDING GRANT on `pi4-regression.spec`: EIGHT directives, all landed, anything further back to this
seat — under the condition that saved this round: OBSERVE THE LEG BEFORE WRITING THE RULE.**

**Peer push predicates at this writing (monotone half only; counts belong to their seats):**
`origin/hw-jetson = 06ffdaf8` — orin's `42eb2736` and `52a66ab8` both **owed**.

### 2026-09-07 · **`:359 OPTIONAL K1-atr` IS SILENTLY UNFAILABLE AND NOW LOAD-BEARING** (answer to orin 20)
Kind census, both scored pi specs:

    pi4-regression.spec   REQUIRE 118 · COUNT 2 · FORBID 134 · PENDING 0 · OPTIONAL 1 · COMPLETE 2
    pi4-barename.spec     REQUIRE   3 · COUNT 0 · FORBID   3 · PENDING 0 · OPTIONAL 0 · COMPLETE 0

**Zero PENDING. One OPTIONAL, and it is the hole.** `:359 OPTIONAL K1-atr:.*codec PASS`, sitting between
two REQUIREs in the same K1 block (`:357` persist, `:358` corrupt). What it guards, off pi's own wire:

    :: K1-atr: UNAFS.ATR owner/grants format M1 — codec PASS (16-row bound, per-row CRC fail-closed,
       binding-checked), on-disk helpers disk PASS …

**That is the `owner`/`grants` codec this round's ACL work reads** — `native_write_authz` does
`ino.attributes.get("owner")` (`fs/vfs.rs:892`) and `grants:{principal}` (`:900`). **`vfs.aclsym` depends
on a codec whose only guard cannot fail.** Codec regresses → `K1-atr` goes quiet → run stays green → the
new ACL rows evaluate attributes that silently stopped decoding.

**Observe-first already satisfied:** `K1-atr` fires **1×** in pi's capture, same as `K1-persist` and
`K1-corrupt`. It has been passing, not skipping — it simply could never have said otherwise.

**ACTION (offered to orin 20 as grant #9, else this seat's focus turn): promote `:359` OPTIONAL →
REQUIRE**, red-proofed first, with a provenance comment recording *why* — optional when attributes were
a side feature, load-bearing once the ACL rows landed. **Floor 126 → 127.**

⚠ **COMPLETE (`:99`, `:100`) IS A FALSE LEAD — do NOT promote.** Non-failable **deliberately and
compensated**; `mbench.py` says so itself: *"COMPLETE deliberately answers True … so it can never be
counted into the `got/len(req)` witness tally"* and *"an absent marker is the TRUNCATED verdict (rule 2
of `run_verdict`), which is neither a pass nor a regression."* Promoting them converts a truncation
signal into a regression signal and destroys the distinction.

⚠ **THE HOLE IS IN THE GATE, NOT ONLY THE AUDITOR.** orin sourced it to `orin-specscore.py`'s `failable`;
it is equally true of **`mbench.py`** — `:188-196`, `failed()` returns `False` for anything not
REQUIRE/COUNT/FORBID, `satisfied()` returns `True` for everything but REQUIRE/COUNT. **PENDING/OPTIONAL
are unfailable in the program `arroyo` actually gates with.**

**Small find, rmbp's lane:** `mbench.py`'s docstring still says *"the pi4 gate's PASS is 63/63"* — stale;
the floor is 120 today, 126 after the fold.

**orin 20's two traps, worth carrying:** a witness called from inside the thing it is meant to
discriminate is **not** a discriminator (`load_witness_poll` inside `run_capstone_boot_core`); and a lone
`tick 1` must NOT pass — it is the IRQEL-RT one-shot's signature, i.e. the BUG. Requires ≥2 distinct N.
**orin 20 holds `MBENCH 125/125` as the pi4 floor and treats a move as a STOP; `jetson-sync1.spec` only,
`pi4-regression.spec` untouched by that seat.**

### 2026-09-07 · **K3 does NOT need the RO seam — and PI IS THE EXISTENCE PROOF** (answer to orin 20)
orin 20 flagged `fs/unafs.rs:1483` — *"K3 relied on the RO seam refusing a write to `base_lba`"* — and
asked whether making the Orin's native volume writable would remove a refusal K3 asserts. **No, and the
code says so in the past tense** (`:1481-1487`): *"K4 update: … bit4 **no longer** writes to the volume
(K3 **relied on** the RO seam …; with real writes that would zero the superblock). bit4 instead proves
the now-live write seam is bound-checked."* The dependency was retired when the write seam went live.

**THE SETTLING FACT — pi already runs the configuration orin is proposing:**

    :399  REQUIRE K4-write:.*clean-tree PASS      ← pi's native volume is WRITABLE, and gated
    :400  FORBID  K4-write:.*FAIL
    :392  REQUIRE K3-mount:.*byte-verified PASS   :378 REQUIRE K3-revoke:.*durable-first PASS
    :1961-1963  three paired FORBIDs
    wire: :: K3-mount: native unafs volume located (base_lba=114688, 2048 blocks) + superblock v5
          mounted + ls/cat byte-verified PASS [w=0x1ff] ::     (all three fire, 1× each)

**`base_lba=114688, 2048 blocks` is byte-for-byte the Orin's span** (`part=[114688..131072)`,
`span_blocks=2048`). **Identical geometry. Pi's is writable with K3 green; the Orin's is RO.** So the
posture change closes a divergence rather than removing a protection — METAL-CONFIRMED 2026-07-12.

⚠ **Caveat given: the retirement was done on PI'S path.** orin must grep the Tegra path for any fixture
still writing to `base_lba` expecting a refusal — a Tegra-side bit4 equivalent nobody re-pointed would
zero the superblock on first write. **A negative result in this tree bounds this tree only.**

⚠ **AND THE SIZE HALF IS PROBABLY UNNECESSARY — decouple it.** orin wants to grow the volume "so `/` is
actually usable as a root". **Pi mounts `/` on `NativeBackend` at 2048 blocks = 1 MiB today**
(`shell.rs:5409`) and runs its entire ACL / revoke / persist / write battery on it. **1 MiB is
demonstrably enough.** A staging-side size change is the riskier half and must not ride along as if the
binding fix required it.

**`fs/unafs.rs` grant: NOT acked — shape only, no diff yet.** When the census lands, read `:470-473`,
`:491`, `:1506`, `:1525`, `:1614` against this board's bounds before acking. **`emmc2.rs:703` defended
as-is: the census must keep DERIVING.**

**Grant #9 (`:359` promotion) stays on THIS queue** — orin 20 declined for mechanical reasons (nine
executors gating on `125/125`; a floor moving to 126 mid-fleet arrives as nine gate failures). Correct
call. Goes in at the focus turn with its provenance comment, or on their ask after their fleet lands.

### 2026-09-07 · ⚠⚠ **PI'S K3/K4 GREEN IS QEMU EVIDENCE — this seat called it metal-confirmed and should not have**
Correction issued to orin 20. pi's live K3/K4/K4-write greens come from `unaos/target/serial-pi.log`, a
`kernel8-test` capture with **zero `Loaded 'kernel8.img'` anchors** — QEMU, not metal. The
"METAL-CONFIRMED 2026-07-12" is the **spec's own dated comment** (`pi4-regression.spec:388-390`),
inherited and relayed, not re-derived. **The Pi has no on-card log, so it cannot be re-verified without a
bench turn.**

⭐ **AND THE PI HAS ALREADY HAD ORIN'S EXACT SYMPTOM ON METAL** — `arch/aarch64/sched.rs:46-55`:

    [spin6] cpu=2 REFUSING corrupt switch-in: task=70:u7-launch ctx_sp=0x20c9e70
    outside its stack [0x20ca000,0x20ce000) — the parked frame was OVERWRITTEN

`u7-launch` **dropped on Pi METAL**, `ctx_sp` 144..928 B below the task's own 16 KiB low bound — the
launcher's own chain running off the bottom of its stack. `u7_launcher` is ~47 no-argument calls whose
callees inline into its single frame (`syscall.rs:16220-16222`), the exact shape that exhausts a stack
silently; **SPIN-6 refuses the switch-in and every leg past it is called and never reached.**
**FIX, in tree: `main.rs:756` `const U7_LAUNCH_STACK_SIZE: usize = 32 * 1024`** — that one task at 32 KiB
vs the default 16 KiB (`sched.rs:41`), "1.84x the static worst case, 2.66x the QEMU peak".

**OWED TO PI'S OWN FOCUS TURN: is 32 KiB still right?** The chain has grown since (SHELLRELICS, ACLSYM,
xvol legs all hang off it). **Watch `[u7stk] … headroom=` on the gate, per S13** — and note S13's sibling
point: an unreached launcher is also an UNMEASURED one, because `[u7stk]`'s only caller is inside it.

**Told orin 20 to check `awk '/spin6/'` and `[u7stk] headroom=` FIRST** — it explains "called
unconditionally, zero output" with no cfg involved.

**THE RULE, sharper than this seat stated it:** *shared code is not shared execution* — **and one board's
QEMU is not even that board's metal.** Pi's green certifies pi's QEMU execution only.

### 2026-09-07 · ⭐ **A PLAIN `./arroyo kernel8` METAL IMAGE RUNS NO U-SERIES CASCADE — by design, documented**
Corroborates orin 20's witness-polarity candidate from pi's side. The whole U-series sits behind one gate:

    main.rs:527   #[cfg(feature = "witness")]
    main.rs:528   if let Some(&cpu) = online.first() {   ← tlb-warm, u5/u6/u7-launch …
    arroyo:5210   local K8_FEATS="baremetal,skip_xhci"   ← the DEFAULT metal image
    arroyo:5215   K8_FEATS="${K8_FEATS},witness"         ← CONDITIONAL

And the tree says it outright — `main.rs:5686`: *"`witness`-gated exactly like every other `[u7stk]`
probe — so **the plain `./arroyo kernel8` media build carries none of it** while `kernel8-test` and the
metal witness image carry it unconditionally."*

⚠ **CONSEQUENCE FOR PI'S OWN METAL WORK: unless a Pi card is staged with `UNAOS_WITNESS=1`, the flown
image runs NO K1/K3/K4/U-series legs at all** — so a Pi metal boot proves nothing about any of them, and
the board has no on-card log to say otherwise. **Check `strings` on the image AS STAGED before scoring
any metal claim against these fixtures** (CLAUDE.md's reachability rule; ⚠ never `strings -n 4` — it
breaks at em-dashes; use `grep -a -o` at the site). **A missing `⚡ kernel features:` banner cannot
testify about its own image.**

### 2026-09-07 · **S13 IS NOT REFUTED — orin 20 observed its FIX; and one present-tense error was pi's**
orin reported 64 `[u7stk]` hits from `jd2-console` and read it as refuting S13's *"the probe had no
reachable caller outside `u7_launcher`"*. **Same shape here:** `stk_probe` callers outside the launcher
are `main.rs:4668` (`stackpool_stk_probe`, the shared pump-path body), `:5686` (`render:pass`), `:7527`
(`pumppath:pass`) — all `#[cfg(feature = "witness")]`.

**But S13's ledger status is `landed`, and its closure text IS the fix that added them** (`stk_probe_bounds`
`a20839c6` + the `orin-render:pass1/2` seed `01739a93`). **The finding was past tense; the observation
confirms the closure.** Told orin not to file it as an S13 correction — **a landed row re-opened as
"wrong" costs the next seat the fix's provenance.**

⚠ **What WAS wrong is pi's own sentence:** *"an unreached launcher is also an unmeasured one"* — true
before `a20839c6`, false after, and stated in the present tense. **A closed row's finding does not stay
quotable as a live fact.**

### 2026-09-07 · orin 20's refutation of pi's SPIN-6 hypothesis — clean, and it enlarged the finding
`spin6` hits=0, `u7-launch` hits=0 (a SPIN-6 refusal would have NAMED it), `u5/u6-launch` hits=0,
`wcb_launcher` hits=0; tasks spawned are `boot-core · jd2-console · el0-hello ×2 · orin-render ×2`.
**Never spawned, not refused.** So pi's 32 KiB `U7_LAUNCH_STACK_SIZE` is not implicated — **and the Orin
cannot answer pi's "is 32 KiB still right for a grown chain?" question, because that board never
exercises the chain. Pi's question stays pi's, unanswered.**

**Grant #9 reframed (orin's point, and it argues FOR the promotion): the ATR codec is load-bearing on a
board that does not execute its witness.** Promoting `:359` makes pi **the only board where an ATR codec
regression is catchable at all** — the difference between one board catching it and none.

### 2026-09-07 · `[spread4]` — **not load-bearing on pi, and the SHARED FILE IS INNOCENT**
orin 20 reported that `arch/aarch64/sched.rs`'s claim *"`[spread4]` is LINK-TIME DEAD on a tegra build"*
is contradicted by 1212 `[spread4] live` lines on their flown wire, and asked whether pi's tree carries
the same stale premise. **Checked — it does not, and neither does the shared file.**

**The source claim, `sched.rs:3310-3312`, is ALREADY configuration-scoped:** *"IS NOT REACHED ON A TEGRA
BUILD: `LC_ALL=C grep -a` over the linked **`arm-tegra-el0`** kernel finds no `[spread4] live` at all …
because every caller of `spread4_witness` is unreachable **on that configuration**."* It names the
artifact, the method, and ends in "on that configuration." **No edit needed; routing a correction would
be editing correct text.**

⚠ **THE DRIFT IS IN THE COPY.** `jetson-sync1.spec:379-382` restates it as a property of *this board*.
**Source said CONFIGURATION, copy said BOARD.** **A NEW SHAPE, distinct from the round's other four: not
"a check that could not tell us" — a claim CORRECTLY QUALIFIED AT ITS SOURCE that LOST THE QUALIFIER IN
THE RESTATEMENT.** The original author did the work; the paraphrase spent it.

**Likely resolution, given orin's own witness-polarity finding: both can be true if render9's flown image
is not the `arm-tegra-el0` configuration** — test which linked artifact it actually is before concluding
the comment is wrong.

**Pi's position:** `[spread4]` is **not scored** in either pi spec (one comment mention,
`pi4-regression.spec:1743`, `[spread4] d1=` as a population); **and it DOES appear on pi's wire (2 hits)**
— which makes **pi the CONTROL for orin's experiment**: same shared code, the other configuration, string
present, exactly as a claim scoped to `arm-tegra-el0` predicts.

### 2026-09-07 · TWO NORMS WORTH KEEPING, from orin 20's own errors
- **A warning about a trap can be written in a form that IS the trap.** orin's brief warned that
  `:: SCHED: load ::` is not a discriminator — **and misspelled the token** (real: `:: SCHED: load c0=…`,
  1216 hits; as written, zero). A scorer built faithfully from it would have carried a dead pattern into
  a flight. **The control token is what makes a warning survive being written down wrong.**
- **Closed rows are evidence about the PAST; observations are evidence about NOW.** orin's words:
  *"I would have destroyed the audit trail of a closed defect to record an observation that confirms
  it."* **Filing the second as a correction to the first destroys the only record of why the fix exists.**

### 2026-09-07 · **A CITATION THAT NEVER RESOLVED — not drift, and the diagnosis changes the fix**
orin 20's `jetson-sync1.spec:379-381` cites `sched.rs:3223` for the `[spread4]` qualifier and filed it as
citation DRIFT. Measured on this branch:

    claim "IS NOT REACHED ON A TEGRA BUILD" at:  8131cd2d → 3310   87006a18 → 3310   HEAD → 3310
    sched.rs:3223 at those same shas:            ///               ///               ///

⚠ **Bound stated to orin: these are all `hw-pi4` shas and `sched.rs` is shared, so their branch's history
may differ — check at their own shas.** But on every reachable ref, `:3310` is the claim and `:3223` is a
bare doc line. **The citation was wrong when written, not decayed.**

**Why the distinction is load-bearing:** *drift* ⇒ line numbers decay, fix is discipline about mutable
files. *Never-resolved* ⇒ **the citation was not checked at authoring time**, and the fix is that
**a citation is a claim like any other and gets verified when written**, not treated as bookkeeping
attached to a real claim.

⭐ **AND THE TWO FAILURES STACK ON ONE CLAUSE:** the qualifier was **demoted into a parenthetical** *and*
that parenthetical's pointer **never worked**. **The one piece of text that would have corrected the
over-generalisation sat behind a pointer that has never resolved.** A reader doing the diligent thing —
stop at the parenthesis, follow the citation — lands on a bare `///` and concludes there is nothing there.

**orin's refinement, better than this seat's reading and adopted:** the copy did not LOSE the qualifier,
it **DEMOTED** it. *"A qualifier demoted into a parenthetical is load-bearing text in the one place
nobody quotes from."* **Different fixes: "restate the qualifier" vs "a qualifier belongs in the
ASSERTION, never in its citation."**

**Four citation failures, four mechanisms, four seats, one day:** rmbp resolving a line against the wrong
arch · pi inheriting a dated comment nobody re-measured · orin's `head -20` truncation · a bare line
number that never resolved. **File + symbol, always.**

### 2026-09-07 · the free control — **the fleet's second board is an experiment control that costs nothing**
orin 20's line, worth keeping: *"A two-board fleet gives you a control for free, and only if the seats
talk."* Pi was the positive control for their negative `[spread4]` result — same shared code, the other
configuration, string present — and **neither seat designed it that way**; it fell out of one message
asking. ⚠ **Corollary: an unasked control is an invisible one.** When a claim is configuration-scoped,
the other board is the cheapest possible test and it is only reachable by talking.

**Also affirmed to orin: a whole-file count over a rolling capture is not a measurement of a boot.**
Segment by `Loaded 'kernel8.img' … size 0x…` anchors first — the same rule that makes pi's own
`serial-pi.log` numbers QEMU-only (zero anchors). **"Nonzero" is the honest form until windowed.**

### 2026-09-07 · ⚠ **REVERSED: the `sched.rs:3223` citation WAS correct when written. This seat's sample could not have falsified it.**
orin 20 found the authoring sha: `1b25e981` (2026-08-25), where the claim sat **at line 3223**. The file
then grew ~87 lines above it. **All three shas this seat sampled POSTDATED the authoring commit, so none
could have shown the pre-drift state.** The earlier entry's "never resolved" conclusion is **withdrawn**.

**THE METHOD LESSON, and it is the sharpest of the day: a sample must be SELECTED TO CONTAIN THE
FALSIFIER, not merely be numerous.** Three data points, zero discriminating power — and it *felt* like
diligence; the bound was even stated correctly while the sampling could only confirm. **For a
line-number claim the falsifier lives at the authoring commit: `git log -L <n>,<n>:<file>`, one command.**
Same family as a truncated `head -20` and an unwindowed count, in its most respectable costume.

**The author is VINDICATED** — this seat nearly recorded a discipline failure against someone who
committed none. **The S13 hazard aimed at a person instead of a row.**
**What survives: the qualifier is still DEMOTED into the parenthetical.** Two real failures, one the
author's and one the medium's; **only the medium's is fixable by discipline — file + symbol.**

### 2026-09-07 · **PI HAS NO KELF-STYLE ANCHOR GAP — 9 anchors, 9 boots, measured**
orin 20's archive holds 74 coldboots but only 21 `KELF` lines, which would void the segmentation rule for
most of their retrospective evidence. **Checked pi's side:**

    ~/unaos-bench/capture/line-acm0/pi.log   40,248 lines · 9 "Loaded 'kernel8.img'" anchors
    segmented by those 9, per-boot control marker in each segment:
      seg 0 (BEFORE the first anchor): 3 lines, MMU on = 0   ← no un-anchored boot at the head
      seg 1..9: MMU on = 3 in every single segment           ← uniform

**Nothing outside an anchor. The segmentation rule stays applicable to every Pi boot on disk.**

⭐ **And pi's anchor beats KELF structurally: it carries `size 0x…`, so it identifies the IMAGE, not just
the fact of a boot.** Nine boots span ≥5 distinct sizes (`0x148ba0` `0x16bce8` `0x16c830` `0x24e750`
`0x250088` `0x279e90` `0x27ae10`), and the small-image segments have a different marker profile
(`BCM2711`=0, `UnaOS`=1) from the large ones (4–5) — **so the anchor separates knob-off builds from armed
ones without reading a banner.** The property that makes it work: **emitted by FIRMWARE, not by the
kernel whose identity is in question.**

### 2026-09-07 · ⭐ **"COMPUTED, PRINTED, NEVER COMPARED" — one defect class on two boards**
orin's flight defect: `root_probe()` reads the root volume's FAT label, **prints it, and does not test
it** — boot medium `0xde001a13` and root medium `0xabfbdefa` sit four lines apart on one wire, uncompared
(`boot_volume_serial()` is `cfg(x86_64 + sdhcblk)`, so aarch64 discards a value the loader already
passes).

**PI'S QUEUE ITEM 3 IS THE SAME SHAPE:** the compositor verifier already emits `moved=` (`wm.rs:6682`)
and `pi4-regression.spec` **never consults it** (`:576` REQUIRE / `:577` FORBID both ignore the field).
**The expensive half is done and the cheap half — the comparison — was never written.**

**Proposed as a SHARED row rather than two arch rows, with a natural gate leg:** *for every witness field
a spec never references, either reference it or declare it diagnostic-only.* ⚠ **And the same discipline
item 3 is blocked on applies to orin's fix: a comparison added before the values are trustworthy
manufactures a red.**

### 2026-09-07 · ⛔ **CLAUDE.md's LOG-INSPECTION IDIOM IS UNSAFE FOR THE PROJECT'S OWN TAG CONVENTION**
orin 20 retracted every `[spread4]` number after three compounding mechanisms; one was
`awk '/[spread4] live/'` — an unescaped bracket read as a CHARACTER CLASS. **Generalised and measured on
pi's own log** (`unaos/target/serial-pi.log`, ~78k lines):

    tag         awk '/\[tag\]/'   awk '/[tag]/'    awk 'index($0,"[tag]")'
    [u7stk]           219            77883                 219
    [pstrip]           44            77903                  44
    [wc-d]             19            77876                  19

⚠ **`gawk` HERE PRINTS NO WARNING** (checked stderr explicitly) — orin's awk warned and was ignored;
this machine fails silently. ⚠ **And the error goes BOTH WAYS** — over-count here, under-count there —
so **no eyeball defence exists.**

**`CLAUDE.md` says *"Inspect serial logs with `awk '/pattern/'` — not `grep`"*, and every witness tag in
this project is bracketed** (`[wc-d]` `[u7stk]` `[pstrip]` `[spread4]` `[orinrast]` `[clickroute]`
`[sdmmc]`). **The documented idiom is exactly the unsafe form for the project's own naming convention** —
right about `grep`, wrong about the shape everyone actually searches for.

**FIX: `index($0, "[tag]")` — literal, no escaping, no dependence on an awk build's warnings.** Raised to
orin 20 to route with rmbp (shared CLAUDE.md; naming rather than editing). **OWED: confirm it landed.**
Memory: [[awk-bracket-tags-need-index]].

**And the framing worth keeping from orin's retraction:** a stale artifact, a mis-windowed range and a
silently-broken pattern **all produce a NUMBER** — the most persuasive-looking thing a wrong method can
hand you. Their control token (`TEGRA-UNAFS`, known-present) is what caught it.

### 2026-09-07 · ⛔ **UNIT ERROR CORRECTED: 2048 blocks is 8 MiB, NOT 1 MiB — the baton was wrong and this seat repeated it**
Measured in-tree: `unafs/src/storage.rs:30` `pub const BLOCK_SIZE: u64 = 4096`;
`adapter.rs:48,51` `SECTOR_SIZE = 512`, `SECTORS_PER_BLOCK = BLOCK_SIZE / SECTOR_SIZE // 8`.
`PartitionSpan::block_count = sector_count / SECTORS_PER_BLOCK`, so the span `[114688..131072)` =
16,384 sectors ÷ 8 = **2048 blocks × 4096 B = 8 MiB**. Corroborated by `sdmmc_tegra.rs:1467`:
*"512 MiB = 131,072 blocks"* (512 MiB / 4096 = 131,072 ✓).

**The pi-8 baton's "2048 blocks = 1 MiB" is a unit error, and this seat relayed it to orin as
"1 MiB is demonstrably enough."** The *conclusion* survives — pi runs `/` on this volume with its whole
ACL/revoke/persist/write battery — but the number was wrong by 8×. ⚠ **And a third value is in play:
`block_count` describes the PARTITION; the VOLUME inside it may be smaller (orin measures 4 MiB), and
NOTHING on the read-only probe path prints the volume's own size.** A quantity three seats reasoned about
all day that **no instrument reports.**

### 2026-09-07 · ⛔⛔ **`flash-pi4.sh` VERIFIES THE IMAGE AND NEVER THE TARGET — it will overwrite any card in the slot**
orin 20 warned "do not use `flash-pi4.sh` for the Orin card — it is how the Pi image got there in the
first place." **Inspected `~/unaos-bench/tools/flash-pi4.sh`; the hazard is pi's too.** Its guards:

    :9   DEV=/dev/mmcblk0                      ← hardcoded
    :14  refuse if $DEV absent
    :17  refuse if $DEV is not a block device   ← the "impostor" file check
    :25  refuse unless sha256($IMG) matches its MANIFEST line
    :32  sudo dd if="$IMG" of="$DEV" …
    :36  cmp -s -n 1048576 "$DEV" "$IMG"        ← read-back of the WRITE

**Zero hits for `label`, `blkid`, `UNAOS-PI`, `UNAOS-ORIN` or any read of the target's EXISTING content.
It verifies WHAT IT WRITES and the device's TYPE — never the device's IDENTITY.** So whatever card sits
in `/dev/mmcblk0` receives the Pi image, and the script cannot tell a Pi card from an Orin card from a
stranger's. **That is exactly how the Orin's card became a repurposed Pi 4 card.**

⭐ **This is [[verify-what-booted-not-what-you-wrote]] on the WRITE side: a 10/10 card sha proves the
WRITE, not the TARGET.** The post-write `cmp` has the same blind spot — it proves the bytes landed, never
that they landed on the right card.

**OWED (bench/tooling, pi's lane): add a TARGET-identity gate to pair with the existing image-identity
gate** — read the card's existing BPB label / partition signature and refuse on mismatch unless
explicitly overridden. **Peter's call before any change to a flashing tool** (bench hardware is his).

### 2026-09-07 · ⭐ **THE UNIT WAS DOCUMENTED AT THE SITE — a summary beat the source for a full day**
`fs/unafs.rs:470-471`, present the whole time:

    // Does the mounted volume fit inside that partition? `block_count` is 4096 B blocks; the
    // partition is counted in 512 B sectors, so eight sectors per block. Checked, not assumed.

**The comment states the unit, states the conversion, and asserts its own epistemic status — and three
seats propagated the baton's "1 MiB" for a day without opening it.** Not an undocumented quantity: **a
documented one that lost to a summary.**

**GENERAL FORM, worth more than the correction: a summary artifact — baton, resume, THIS QUEUE FILE —
outcompetes the source for attention precisely because it is easier to read.** Mirror of ledger S16
(invariants in comments enforced by nothing): here the comment WAS adequate and nobody consulted it.
**The fix is not "write it down" — it was written down. It is that an inherited NUMBER gets SOURCED
before it is relayed**, exactly like an inherited claim. ⚠ **This file is the same hazard: whoever reads
it at the focus turn must re-source any number before acting on it.**

### 2026-09-07 · ⚠ SCOPE: **the Orin's next card write is protected; PI'S IS NOT**
orin 20's `load-card10.sh` (target identity first, 27/27 dry-run, refuses on a reproduced `UNAOS-PI` /
`0xabfbdefa` fixture) is **Orin-scoped and lives in their scratch.** Checked pi's side: **no pi flash path
does a target-identity check** — the only `~/unaos-bench/tools/` files referencing a label at all are
`x86-media-wake.sh` and `bench-state.sh`, neither of which is the Pi write path.

**That asymmetry is what produced the repurposed card: a protection built where the incident was noticed
rather than where the class lives.** Pi's target-identity gate stays OWED — bench tooling, named to Peter,
**not to be read as fixed by orin's dry-run.**

### 2026-09-07 · ⛔ **`fits=yes` CANNOT READ `no` — vacuous on pi too, and three seats quoted it as evidence**
orin 20's finding, confirmed here from the shared source. `unaos/libs/fs/unafs/src/adapter.rs:614`,
feeding the `PartitionSpan` returned at `:623-626`:

    let block_count = p.sector_count / SECTORS_PER_BLOCK;    // ← the SAME partition entry

and `fs/unafs.rs:472-475` then evaluates `span.block_count.checked_mul(8) <= p.sector_count` **against
that same `p`** — i.e. `(sector_count / 8) * 8 <= sector_count`, **true for every input under floor
division.** It can only fire if two MBR parsers disagree. It is in the baton's headline finding and in
this seat's own messages all day: `:: PART: unafs span check — slot=2 … span_blocks=2048 fits=yes
magic=ok ::`. **`magic=ok` is real. `fits=yes` has never been able to fail.**

⭐ **BUT THE PROPERTY IS CHECKED ON PI — by a comparison nobody quotes.** `fs/unafs.rs:1525`, K3 **bit3**:

    // bit3: the volume fits the partition that carries it.
    if fs.superblock.block_count <= span.block_count { r |= 1 << 3; }

**SUPERBLOCK (read from the volume) vs SPAN (from the MBR) — two independent sources, genuinely
failable.** Exercised here: pi's K3 line reads `[w=0x1ff]`, bits 0..8 all set, **so bit3 evaluates and
passes on every Pi boot in the capture. The defect is the WITNESS, not the property.**

⚠⚠ **AND THE REASON ALL THREE SEATS CITED THE WRONG ONE IS STRUCTURAL — a NEW axis:**
**the vacuous check prints a human-readable word (`fits=yes`); the sound check is bit 3 of a hex mask
(`w=0x1ff`).** Nobody quotes a hex mask; everybody quotes `fits=yes`. **LEGIBILITY DRIVES CITATION, NOT
SOUNDNESS.** Not scope, time or observability — the same mechanism as a summary beating the source
([[claims-relayed-past-their-check]]), one layer down.

**OWED (small, `fs/unafs.rs`, rmbp's file):** either make `fits=` compare the superblock the way bit3
does, **or rename it to what it reports** (`span_ok=`, a geometry sanity line). **The current name
promises the guarantee bit3 delivers.**

### 2026-09-07 · **"CORRECTION BY ACCRETION" — worse than stale prose, and it needs the name**
orin 20: `c3abd946` **added** a section saying an item happened and **left the contradiction standing 47
lines above its own refutation** — same file, same commit, same author. **Not a claim nobody revisited: a
claim revisited, corrected, and left beside its correction.** A reader who stops at the first hit gets the
false version, and the document is internally consistent nowhere. **Distinct from S17/S28 stale prose and
from SP3's false-absence.**

**Also from that thread — three artifacts today that PRESENTED AS MEASUREMENTS AND WERE NOT:** a
timestamp (a computation with a timezone in it — orin's `-0600` retraction-of-a-retraction), a symbol
name (looks checkable), and `fits=yes` (looks like a comparison). **Each was quoted precisely because it
presented well.**

### 2026-09-07 · ⭐ **`fits=` SHAPE ANSWER: MAKE IT SOUND, DO NOT RENAME — and the reason is a build fact**
rmbp 15 granted the `fits` fix in advance (`fs/unafs.rs` is theirs) with **pi's ack as condition 3 of 4**,
"because their board runs bit3 every boot and is the only place the sound check currently executes."
Asked which shape. **Answer: make the printed field mean its name.** The deciding fact, traced here —
both emitters are single-caller:

    fs/unafs.rs:404    partition_witness(handle, &span)   ← called from mount_on(): THE MOUNT PATH
    syscall.rs:16448   k3_mount_selftest()                ← called from u7_launcher

and `u7_launcher` spawns **only** from `main.rs:527`'s `#[cfg(feature = "witness")]`, while
`arroyo:5210` builds the default metal image as `baremetal,skip_xhci` (witness appended conditionally at
`:5215`).

⭐ **So on a plain `./arroyo kernel8` image — the one that goes on a card — `fits=` prints and bit3 NEVER
RUNS, on BOTH boards.** bit3 is not the sound alternative to `fits=`; it is the sound version **in the
one configuration that is not flown**. **Renaming would leave EVERY METAL BOOT ON EVERY BOARD with no
check at all** — not just the Orin, which is how this seat first framed it and it was understated.

**Duplication with bit3 is a CONTROL, not a cost:** two independent evaluations, one on the mount path
and one in the selftest; **if they ever disagree that is itself a finding**, and it is the only
cross-check available between a witness build and a media build. **Keep both; do not fold bit3 in.**

**Falsifier discipline sent with the answer: mutate the SPAN OR THE SUPERBLOCK, never the comparison.**
Editing the comparison proves the branch prints; it does not prove the operands are independent, **which
is the whole defect.** The clean fixture is a staged volume whose superblock `block_count` exceeds its
partition — the sound check reds while the old one still says `yes`, proving the fix and the vacuity in
one run.

**Independence sentence for the site (condition 2):** `superblock.block_count` is read **from inside the
volume**; `span.block_count` is derived from the **MBR partition entry** (`adapter.rs:614`,
`p.sector_count / SECTORS_PER_BLOCK`) — different media structures, written at different times by
different tools. **That sentence is what the old check lacked, and writing it would have exposed the
vacuity, because the old form takes both sides from `p`.**

⚠ **PI'S ACK IS PRE-GIVEN ON THE SHAPE ONLY; the formal ack waits for the diff.** On arrival check: both
operands genuinely different sources · falsifier mutated DATA not the comparison · **pi's K3 mask still
reads `w=0x1ff`** (bit3 must not be disturbed by a change to its sibling) · today's card measured.

**Method note worth keeping: orin derived the vacuity from `adapter.rs:614` FORWARD, rmbp from
`sdmmc_tegra.rs:1829`'s sizing guard BACKWARD, neither relaying the other, same result.** After a day of
three seats propagating a unit error and a phantom symbol by relay, **deriving from the opposite end and
comparing is the answer to "how do we stop relaying."**

**rmbp's form of the legibility axis supersedes pi's — use theirs:** *"The legible instrument is not
broken."* A stale comment is wrong; `fits=yes` is **right and irrelevant** — computing exactly what it
computes, correctly, every boot, answering a question nobody asked. **Nothing about it malfunctions,
which is precisely why three seats read past it.**

### 2026-09-07 · ✅ **`fits=` FIX ACKED — condition 3 of rmbp's grant satisfied, with one doc amendment**
Patch: `~/unaos-bench/scratch/orin20/fitswit/`. Touches `fs/unafs.rs` + `docs/dev/OS/09_FILESYSTEM/partitions.md`.

**VERIFIED HERE, not accepted on report:**
- **The builtin-FORBID claim — load-bearing, because it is what gives pi enforcement with NO spec line:**
  `mbench.py:135` `DEFAULT_FORBIDS = [r"-> FAIL", r"FAIL ::", r"PANIC"]`, installed `:264-265`.
  **`=> FAIL ::` matches `FAIL ::` ✓.** The ack would have been refused if this had not held.
- **Falsifier, read from their log:** `❌ FORBID hit @ line 149: … volume declares 4096 blocks but MBR
  slot 2 carries only 2048 => FAIL ::` · `❌ MBENCH FAIL — 125/125 required witnesses, 1 forbidden hit`.
  ⭐ **All REQUIREs still pass; the only failure is the forbidden hit** — the mutation reds exactly one
  thing by name and disturbs nothing else, which also discharges the `w=0x1ff` condition indirectly
  (`REQUIRE K3-mount:.*byte-verified PASS` is inside that 125). **Data mutated (4096 = 1024×4), never the
  comparison.**
- **Card measured:** `sb_blocks=1024 span_blocks=2048` — 4 MB volume in an 8 MB partition, by `od` on a
  freshly built volume. **The third and final correction of the baton's "1 MiB", and the only one sourced
  from the ARTIFACT rather than a comment.**

⭐ **Byte-identity handling exceeded the conditions:** the body went to `span_fit_report` at EOF
**because `panic::Location` embeds source line numbers** — growing a body in place would rewrite location
strings through ~2100 lines below in an arch-neutral module and move bytes in x86 images with no
behavioural change. Line-for-line size preserved. **Tail-append discipline applied unprompted**
([[cfg-does-not-protect-byte-identity]]).

⚠ **THE AMENDMENT REQUIRED WITH THE ACK:** `partitions.md` says bit3 *"never runs on a tegra image"*
(the `tegra_early_stop` `-> !` reason). **True, and TEGRA-ONLY.** bit3 also never runs on **pi's default
media build** — `main.rs:527` `#[cfg(feature = "witness")]`, and `arroyo:5210` builds metal as
`baremetal,skip_xhci` with witness only conditional at `:5215`. **As written, a Pi reader concludes bit3
covers their metal boots. It does not.** Correct form is stronger: **bit3 runs on NEITHER board's flashed
image — tegra by the early-stop terminus, Pi by the witness gate — so on any card that boots, this
witness is the only check of the property.**

**Fifth instance of the day's shape, and the tightest: a claim correctly scoped to the configuration its
author measured, read by another board as a claim about itself — inside the document written to fix a
field that promised more than it delivered.**

**OWED: confirm the amendment landed before treating the ack as discharged.**

---

### 2026-09-08 · pi 9 opens · ⛔ **ORIN 21'S TARGET-IDENTITY QUESTION — ANSWERED, and the answer is "not that shape"**
Sent to orin 21 this turn. **Every claim below re-run in hw-pi4 `059e04db` at 2026-09-08 12:48Z; none relayed
from the pi-8 baton**, which is a summary artifact and therefore the hazard it documents.

**Re-sourced, all three:**
```
git ls-files | grep -c flash                       -> 0
ls -la ~/unaos-bench/tools/flash-pi4.sh            -> exists, 1825 B, Aug 12 20:34
git -C ~/unaos-bench rev-parse --git-dir           -> fatal: not a git repository
```
orin's premise — *"`unaos/scripts/flash-pi4.sh` is pi lane"* — **is wrong on the path. There is no grant to
give.** Shape (b) (a repo gate failing any write path lacking an identity step) **would be vacuous: its
population cannot contain the falsifier.** It sweeps `unaos/scripts/`, finds `identify-card.sh` already
there, passes green, and leaves the only dangerous writer — out of repo — untouched. **(c) observability
disqualifies it before (a) or (b) is asked.**

**READ THE WRITER, don't infer it.** `~/unaos-bench/tools/flash-pi4.sh`, 41 lines: `DEV=/dev/mmcblk0`
hardcoded (`:9`); guards are `-f "$IMG"` (`:11`), `-e`/`-b "$DEV"` (`:14`,`:17`), sha256 vs MANIFEST
(`:26`), then `dd` (`:32`), then `cmp -s -n 1048576 "$DEV" "$IMG"` (`:36`). **Every check is about the
IMAGE and the NODE. Nothing reads what is currently ON the card. The readback proves the write
SUCCEEDED, not that it was LEGITIMATE** — it compares the card to the image that just overwrote it.

⭐ **TWO NEW FINDINGS ON `unaos/scripts/identify-card.sh`, both from reading it, neither in the baton:**

**(i) IT ALWAYS EXITS 0 — stated in its own header: "UNKNOWN is a result, not an error."** So
`if identify-card.sh "$MP"; then …` is **a guard that cannot fire** — green on UNKNOWN *and* on
"no such mount point: …". The verdict is **field 1 of the stdout line**; any wiring must compare the
string (`cut -f1`), never `$?`. [[a-check-that-cannot-fire]] · same family as orin's `root_probe`
printing a label it never tests.

**(ii) MOUNTPOINT vs DEVICE.** `identify-card.sh` takes a mountpoint, default `/Volumes/UNAOS` (macOS);
`flash-pi4.sh` takes a device. Bridging needs a real read-only mount of `/dev/mmcblk0p1` on Linux.
**That is the half that requires a card in the slot — i.e. a bench turn.**

⭐⭐ **THE POLICY MUST BE A DENYLIST, AND THIS IS WHAT DECIDES WHETHER THE REFUSAL SURVIVES.**
"Require PI4 before writing" **refuses a blank card and a freshly-wiped card — both classify UNKNOWN**
(`identify-card.sh` `*)` branch), so it misfires on the most ordinary first flash, and then someone
switches it off. **Correct rule: refuse iff verdict ∈ {JETSON, RMBP}. UNKNOWN warns, never blocks.**
That catches the accident that actually happened (an Orin card repurposed as a Pi card) and is silent on
every legitimate write. **A protection everyone believes in and that has been disabled is worse than
absent** — which is exactly why the wiring waits for a dry-run rather than landing blind.

**OWNERSHIP AS SENT:** pi wires it, **at a FOCUS turn**, because (i) and (ii) both need a real Pi card.
The in-repo gate half is rmbp's lane **and has no population until the bench tooling is in-repo at all.**

⛔ **PETER'S CALL, NAMED NOT DECIDED: the most destructive script in the fleet is unversioned, unbacked
and ungateable.** Whether bench tooling enters the repo is his decision. Joins the two already routed
(the `awk '/pattern/'` idiom; this script's image-not-target verification).

**STATE:** hw-pi4 `059e04db` == `origin/hw-pi4`, **0 unpushed, nothing owed to any seat**
(`git log --oneline origin/hw-pi4..HEAD` empty, 12:48Z). Zero executors, per Peter's standing order
restated this session: *"no new jobs, support orin, and queue your work for your focus time."*

### 2026-09-08 · ⭐ **NEW FOCUS ITEM — THREE FRAGILE FORBIDs IN `pi4-regression.spec`, ONE SCHEDULED TO FIRE**
From orin 21's dead-FORBID composition defect (proven GO-RED on `jetson-sync1.spec`: fitsland's sound
`fits=` separated the tokens unafsgrow's `FORBID span_blocks=2048 fits=` was keyed on — **false GREEN from
two correct changes**). I ran the pass on pi's spec. Scripts: `~/unaos-bench/scratch/pi9/`.

**Counts, from `mbench.parse_spec` itself, not from a grep:** `REQUIRE+COUNT=120` (118+2 — **the floor
derives, it is not quoted**) · **FORBID = 134 declared** (orin said 135) **+ 3 builtin = 137** · COMPLETE=2.

**Junction test** (≥2 `key=` fields joined by a bare space, no `.*` to absorb an inserted field) → 4 of 134.
`:1965` (`:: UVUG: … checksum=`) dropped: different construction (nested negative alternation) and my first
scan **mis-tested it by truncating the pattern** — it needs its own look, not this pass's verdict.

**The other three, GO-RED'd against lines the emitter actually produced, then re-run with one field inserted:**

| spec | pattern | mutated→forbidden | +1 field inserted |
|---|---|---|---|
| `:1549` | `\[pstrip\] rollup samples=… redraws=… skipped=0 srcdelta=0` | **HIT** | **NO HIT** |
| `:1484` | `\[pstrip\] armed .*leds=[0-9] led=` | **HIT** | **NO HIT** |
| `:1132` | `\[wc-h\] .*presspread=[0-9] presspop=([2-9]\|[0-9]{2,}) .*-> AT-RISK` | adjacency verified in emitter (`presspread=3 presspop=3`) | 1 junction |

⛔⛔ **`:1549` IS THE URGENT ONE AND IT IS SCHEDULED. Four adjacent fields, three junctions — the most
exposed FORBID in the spec — and the spec's OWN COMMENT four lines above it says the emitter-side
`paced=yes/no` "remains the real fix and is still owed."** The owed change adds a field to **that exact
line**. Land it anywhere among `samples= redraws= skipped= srcdelta=` and the FORBID guarding the property
goes **silently green**. ⭐ **TAIL-APPEND IS THE SAFE FORM** — the same discipline pi already applies to
byte-identity ([[cfg-does-not-protect-byte-identity]]), for the same structural reason: position is
load-bearing and nothing warns you when you move it.
**FOCUS ACTION: widen the three junctions to `.*` (or re-key on a single field) BEFORE `paced=` lands.**

### 2026-09-08 · **kernel8-test's blind sleep — MEASURED, pi's answer sent to orin (saves their BUILDPERF-2)**
Mechanism confirmed in-tree: `arroyo:6295` `qmp_shoot.py --wait $((secs-1))` (`|| sleep "$secs"` fallback —
blind either way), then `:6325` `mbench.py --replay`. **`kernel8-test` uses NO follow mode.**
Measured with mbench's own `Matcher` over `unaos/target/serial-pi.log` (77,932 lines, PASS):
```
COMPLETE [:: SCHED: task 'el0-midden' -> core] @ 2049 · [:: BANDY-RT:] @ 2163
Matcher.complete() first True @ line 2171 = 2.8%  ->  75,761 lines (97.2%) NEVER READ
0 of 257 directives first-hit after 2171
residual: [click2] x74552 (98.8% idle) · [serfix] 533 · [u7stk] 163 · [sched6] 81 · [prio] 81 · [pstrip] 40
59 distinct tags before, 7 after; ONLY-after = [shellup] x1  ("census t=12908ms …")
```
⚠ **I FELL INTO THE DAY'S OWN TRAP FIRST AND THREW THE RESULT OUT: "FORBID hits after the exit line" = 0 is
VACUOUS. A PASS verdict MEANS zero FORBID hits anywhere**, so on a green capture that check cannot come out
any other way — it is `fits=yes` again, and [[a-check-that-cannot-fire]] again, in my own script.
**What a green capture CAN decide is what RUNS in the discarded window.** That is the block above.

**Verdict: for the spec's declared directives an early exit costs nothing. The residual risk is the three
PHASE-UNBOUND builtins (`PANIC`, `-> FAIL`, `FAIL ::`), live across all 75,761 discarded lines** — a late
panic after `[shellup]` is invisible to a follow that exits at 2171. Policy call, not a measurement.
⚠ **Line position ≠ wall-clock position** — no time saving is quotable from this capture.
⚠ **`[u7stk]` ×163 lands in the discarded window** — that is where owed item #4 (the `U7_LAUNCH_STACK_SIZE`
headroom reading) lives. **A follow-mode exit would remove that instrument.** Weigh it before adopting one.

### 2026-09-08 · **CARDIDGATE STRUCK BY ORIN 21 — the ask is closed, and the LAWS rule that came out of it**
orin verified all three of pi's claims in their own tree before accepting (`ls-files` → 0 at `98213b7f`;
the bench script unversioned; `identify-card.sh`'s "Always exits 0" header, two `exit 0`s) and **struck
CARDIDGATE from their spawn list.** All six points accepted as sent, including the DENYLIST policy and
pi owning the wiring at a focus turn. **They are routing the "does the fleet's most destructive script
enter the repo" decision to Peter this turn, with pi's wording, attributed.** ⛔ Neither seat pre-empts it.

**GOING TO LAWS — orin's wording, pi's sharpening (co-signed both ways):**
> **A check is trusted only when its corpus can produce more than one outcome.** Name the observation
> that would falsify the claim, then ask whether it could have appeared **in this corpus at all**.
> **(a) the falsifier is ABSENT from what was scanned** — a repo gate over an out-of-repo writer; a
> FORBID whose tokens a sibling separated; a guard on an exit status that is always 0.
> **(b) the falsifier is EXCLUDED BY HOW THE CORPUS WAS SELECTED** — a green log cannot exhibit a
> failure; a passing suite cannot show its own gaps; a sample of commits after the authoring commit
> cannot contain the authoring defect. **When (b) holds, change the corpus, not the pattern.**

**(b) is pi 9's addition and it exists because I committed it while measuring the answer to orin's other
question** — orin's wording alone would NOT have caught me: my population was complete, every pattern's
match sites were present, and the count was still incapable of coming out non-zero. **At least three
instances across two seats now** — (a) orin's fold + pi's exit-status guard; (b) pi 9's green-capture
count + pi 8's struck all-postdating sha sample.

⭐ **The escape move, worth its own line: stop asking a green corpus about FAILURES and ask it about
CONTENT.** Not *"did a FORBID hit after 2171"* (one reachable answer) but *"what RUNS after 2171"* (many).
The green log answered the second question completely, and that is the answer that decides the design.

### 2026-09-08 · ⛔ **PETER ASKED THE REPO QUESTION HIMSELF — and the premise both seats carried is WRONG**
His words: *"sounds like the script should be in the repo no? all platforms use it to build a disk image
from now on anyway, correct? since we are creating a partitioned drive w/UnaOS on it?"*
**Measured before answering. Two errors, and correcting them STRENGTHENS his case:**

1. ⚠ **`flash-pi4.sh` DOES NOT BUILD ANYTHING.** 41 lines, one `dd` (`:32`), no `mkfs`/`sfdisk`/`parted`.
   **The partitioned-image builder is `unaos/scripts/make-pi-img.sh` — ALREADY IN-REPO AND VERSIONED**:
   MBR at byte 446 (`:285`), `\x55\xaa` at 510 (`:286`), `mkfs.fat -F 32 -n UNAOS-PI -S 512 --offset`
   (`:288`). **The thing that creates "a partitioned drive w/UnaOS on it" is in the repo already.**
2. ⚠ **NO SHARED SCRIPT EXISTS.** Each platform has its own unversioned bench writer:
   pi `flash-pi4.sh` + `stage-pi4.sh` · x86 `stage-x86.sh` + `x86-media-wake.sh` + `x86-preflight.sh` ·
   orin `orin-card-autowrite.sh` + `orin-card-sync.sh` + `orin-media-wake.sh` + `orin-label-wake.sh`.

⭐ **THE REAL SHAPE, which is a better argument than the one he made: the BUILD half is in-repo and
versioned; the WRITE half is ~6 unversioned per-platform scripts.** The layout is defined in the repo
(VOLID/LAYOUT/FITSLAND/`partitions.md`) and the writers that realize it are not — so the spec and its
only realization can drift with nothing to catch it. **That is the argument for bringing them in.**

⚠⚠ **AND THE PROTECTIONS ARE IN SCRATCH TOO.** orin cited `load-card10.sh` (27/27 dry-run) as why the
Orin's write path is safe. `find ~/unaos-bench -maxdepth 3 -name 'load-card*'` → **one hit,
`~/unaos-bench/scratch/orin11/load-card.sh`**; nothing in `tools/`, `git ls-files` → 0. Asked orin to
confirm the path they tested. **If it is that one, the decision Peter is making changes from "should
bench tooling be versioned" to "the protections we already built are sitting in a scratch dir."**

**PI'S RECOMMENDATION (given, not decided — his call):** bring the WRITE half in as ONE
`unaos/scripts/write-card.sh` taking the target explicitly, calling `identify-card.sh` under the
DENYLIST policy — replacing three per-platform scripts rather than relocating them. ⚠ **Cross-lane by
construction: needs the focus seat or a negotiated three-way grant.** Not started; no jobs running.

### 2026-09-08 · ✖ **PI 9 WRONG #1 — "kill the BUILDPERF-2 measurement." Conceded to orin 21.**
I measured the LINE-POSITION axis (exit at 2171 = 2.8%, 0/257 directives first-hitting after it) and
told orin their raspi4b run was redundant. **It was not: its contribution is WALL-CLOCK stamps —
the exact axis I had written, two paragraphs earlier in the same message, that my capture could not
give ("line position is not wall-clock position; no time saving is quotable from this capture").**

⚠ **THE SHAPE, and it is neither (a) nor (b) of the rule we just co-signed:** the scope note was
CORRECT, STATED, and still did not survive three paragraphs into my own recommendation. **A caveat
that does not propagate is worth nothing** — this is not a check that could not fire, it is a check
that fired and was then talked past by its own author. **Support-seat-specific: the product is other
seats' correctness, so an overreach costs a peer's run, not my own.**
Their opt-in fast-mode shape (inner loops fast, DONE gate keeps the full wall) is right and accepted —
it preserves `[u7stk]` and the three phase-unbound builtins where they matter. With Peter as a rec.

**OPEN, ASKED TWICE, BLOCKING PETER'S LIVE DECISION:** which file orin dry-ran as `load-card10.sh`.
Sole hit on this host is `~/unaos-bench/scratch/orin11/load-card.sh`; nothing in `tools/`, 0 in
`git ls-files`. If that is it, Peter's question changes from *"should bench tooling be versioned"* to
*"the protections we already built are in scratch."* A path is all that is needed; no re-run asked.

### 2026-09-08 · ✖ **PI 9 WRONG #2 — `-maxdepth 3`. And the finding survived it.**
I reported `~/unaos-bench/scratch/orin11/load-card.sh` as the "sole hit" for orin's protective writer.
**It is at depth 4:** `~/unaos-bench/scratch/orin20/cardready/load-card10.sh` (37,579 B, Sep 7 09:11).
**I bounded the population and reported the result as a fact about the host — class (a), my own rule.**
⭐ **What saved it: I quoted the command WITH the cap visible, so orin diagnosed it in one round instead
of a dispute.** That is the entire value of [[comms-predicates-not-values]] in one exchange.
Unbounded re-run confirms exactly two, both in scratch; `git ls-files | grep -ci load-card` → **0**.

### 2026-09-08 · ⭐⭐ **READ orin's WRITER — IT IS ALREADY THE SHARED TOOL. RETRACT "write a new one".**
`load-card10.sh`, **644 lines**: `--target` (explicit, REQUIRED) · `--expect-label` (a **default** of
UNAOS-ORIN, *not* a hardcode) · `--expect-geom` · structured `refuse()` with named check IDs ·
**`RC_REFUSE=1` = nothing was written** · pre-write harvest of the card's existing contents ·
its own corruption fixtures (`:550`). **The generic, parameterised, refusing card-writer EXISTS.**
**Pi's earlier recommendation to author a new `unaos/scripts/write-card.sh` is RETRACTED — promote this
one instead**, and pi's wiring collapses to a denylist call into `identify-card.sh` + a label default.

⚠ **It does NOT write with `dd`** — `udisksctl mount` + `cp -R` (`:382`, `:390`). My "every `dd` writer
on the bench" sweep missed it because my pattern was `\bdd (if|of)=`. **THIRD population-bounding error
of the day — caught before reporting this time, not after.** Accurate line: **the fleet's only raw-device
`dd` writer is `flash-pi4.sh`; the Orin's is a mount-and-copy writer with refusals.**

⛔ **THE FRAMING FOR PETER'S DECISION, and it is sharper than "version the bench tooling":**
**the fleet's BEST-ENGINEERED safety code (644 lines, named checks, refusal codes, self-test fixtures)
sits in `~/unaos-bench/scratch/orin20/`; the fleet's MOST DANGEROUS code (41 lines, one `dd`, no target
identity) is the one that got a stable home in `tools/`.** Exactly inverted. His call, twice-routed now.

### 2026-09-08 · ⚠ **THE MEMORY STORE ITSELF WAS THE DAY'S DEFECT — two live copies, diverging in real time**
Found while filing the day's lesson. **`a-check-that-cannot-fire.md` exists TWICE**, distinct inodes:
```
14384 B  203 ln  06:57:10  …/-home-pmes-src-github-com-pmes-UnaOS/memory/       (canonical)
12871 B  181 ln  06:59:14  …/-home-pmes-src-github-com-pmes-UnaOS-hw-pi4/memory/ (bridge — WHAT THIS PROJECT LOADS)
```
**Each held ONE HALF of today's rule, written two minutes apart:** canonical had pi 9's (b)-instance
write-up; bridge had the co-signed (a)/(b) sharpening. **Neither had both.** Cross-appended, both now
211 lines and carry both halves; nothing removed. `find ~/.claude/projects -name 'a-check-that-cannot-fire.md'`
→ exactly two, so no orin-side third copy — orin told to `stat` before relying on their own record.

⛔ **ROOT CAUSE, and it is the day's class in our own tooling: the bridge `MEMORY.md` asserted "This
bridge dir holds no content of its own." IT HOLDS 35 FILES.** I acted on the sentence instead of the
directory — **I trusted a DESCRIPTION of the store over the store**, which is `fits=yes`, the pi-8
baton's `block_count`, and orin's `flash-pi4.sh` premise, all over again, on the fourth repetition of
the same shape in one day. Line corrected in place with the reason left beside it.
**RULE: before writing a memory, `stat` the file you are about to write AND the one this session reads.**

### 2026-09-08 · ✅ **TRAILING-SPACE BOUNDING IS INVERTED — orin's finding, reproduced BY EXECUTION; pi is clean**
`mbench.py::parse_spec` does `line.strip()` on every row, so a deliberate trailing space is deleted.
Proven here with three one-line specs against the negative-control wire `span_blocks=20480 …`:
```
FORBID span_blocks=2048␣        -> exit 1   FALSE HIT on 20480
FORBID span_blocks=2048\b       -> exit 0   holds
FORBID span_blocks=2048(?=\s|$) -> exit 0   holds
```
**Not merely useless — INVERTED: it turns a bounded key into an unbounded one and reds on exactly the
value the bound existed to exclude.** ✅ **PI SWEEP CLEAN: 0 trailing-whitespace rows in
`pi4-regression.spec` + `pi4-barename.spec`, control = 257 rows ending in non-space** — the zero is
about the data, not the pattern. Nothing owed. orin landed `3894fdc9` on `exec-orin21-rekey` (`:543` →
`span_blocks=2048\b`, four-wire proof) with pi's scan credited; `:2272` deliberately left as one
literal (TF-A firmware emitter — no arc in this repo can interpose a field), documented in the spec.

### 2026-09-08 · ✖ **PI 9 WRONG #3 — I had the MEMORY TOPOLOGY backwards, and I over-stated the alarm**
I called `…/-UnaOS/memory/` "canonical" and reported "two live stores diverging in real time."
**Measured, with orin's `stat` as the second source:**
```
…/-UnaOS-hw-pi4/memory/   36 files   AUTO-LOADED by BOTH the pi and orin harnesses
…/-UnaOS/memory/          25 files   loaded by NEITHER session
```
⚠ **The dir neither session auto-loads is the one that exclusively holds `UNAOS-LAWS.md` AND ALL FOUR
TRACK RESUMES** (pi4/jetson/rmbp/av), plus the baton protocol and hazards. Reachable only by going and
reading it — which works (I `cat`ed the pi4 resume at session start) **but only if you go.**
⭐ **AND THE CORRECTION TO MY OWN ALARM, stated as plainly as the alarm was: the drift surface is
EXACTLY the files present in BOTH stores — that was ONE lesson file (reconciled) plus the index.**
Everything else lives in exactly one store and *cannot* diverge. **This is two stores with different
jobs and one overlap, not a diverging memory.** Topology written into the loaded index for the next
seat, with the rule: **never copy a lesson into the shared dir — duplication is the only drift source.**
⚠ **Consequence worth carrying: a `UNAOS-LAWS.md` edit lands in the dir neither seat auto-loads.**
The (a)/(b) rule going there is seen by a future session only because the baton protocol says to read it.

### 2026-09-08 · ⛔ **BLOCKER RAISED TO ORIN 22 — "Pi bare-metal is NOT changed by this arc" is UNENFORCED**
orin 22 (new focus seat) heads-up: root = the volume the loader was loaded from, matched by FAT serial;
**no fallback when the serial is absent or unmatched — a named boot failure.** Arc `exec-orin22-bootroot`
off `hw-jetson 98213b7f`, changing `shell.rs::vfs_mount_table` "on aarch64 for UEFI-loaded boots",
with the Pi carved out. **Measured at `059e04db` — nothing enforces the carve-out:**
- `vfs_mount_table()` is **`#[cfg(target_arch = "aarch64")]` (`:5405`) — ONE gate, BOTH aarch64 boards.**
  No UEFI-vs-bare-metal discriminator at the definition.
- Body is 11 lines and mounts **`/` UNCONDITIONALLY** (`:5409` `NativeBackend`), then `/fat`, then
  `/usb` if present. **No serial/board/slot/knob in it today** — the changed line IS the unconditional one.
- **Six call sites in `shell.rs`**: `:1375 :1632 :4417 :5174 :5209 :5498`.
⇒ **By their own rule, pi's bare-metal path (no loader, serial absent) lands on the FAILURE branch
unless a guard they have not described exists.** Their reasoning was done over UEFI boots; **the
population that reasoning covered does not contain the Pi** — today's rule, third seat, same day.
⚠ **Documented downstream break:** `unmounted_reserved_volume` (`:5453-5470`) excludes `/` with the
reason written out — *"always mounted and the legitimate fall-through for un-prefixed paths."*
**A conditional `/` falsifies that and re-opens the VFS-4 `-ENOENT` misdirection it was written to fix.**
**ASKED FOR:** (a) name the guard, or (b) `shell.rs::vfs_mount_table` needs pi's ack before landing —
shared kernel core, both aarch64 boards run it. **Design not disputed; the "not changed" claim is.**
⚠ Scope handed over with it: **pi is 94 behind trunk**, their arc is off `98213b7f` — line numbers may
not survive; the structure should. Told them to re-derive at their tip.

### 2026-09-08 · ✖ **PI 9 WRONG #4 — the blocker above ASKED FOR THE KNOB. Withdrawn same turn.**
Peter, immediately after: *"it is an OS booting off an SD card. the card is the hard drive… every boot
we are booting cold… we are writing the drivers to boot cold, boot dumb, and not know or presume
anything about the machine even tho we keep booting the same machine and do in fact know the machine."*
**My ask (a) — "name the guard that keeps the Pi on the current path (a `serial == 0` / no-loader
branch)" — IS a special case, and the special case is what the arc exists to delete.** Withdrawn to
orin 22 in the same turn; told them not to build it.
⭐ **The cold-dumb reading, which is simpler than my blocker:** enumerate, match the serial you were
handed, fail by name if nothing matches. **The Pi having no loader is not a case to code around — it is
a boot path that cannot supply a serial yet, so it fails by name until its loader does. The rule working.**
**Also withdrawn:** the VFS-4 objection. A conditional `/` DOES break *"`/` is always mounted"*
(`:5453-5470`) — **and that is the intended direction.** An always-mounted native root is itself a
presumption. Carry it, don't defend it.
✅ **WHAT SURVIVES, one fact, no ack owed:** `vfs_mount_table()` is ONE `#[cfg(target_arch="aarch64")]`
(`:5405`) over BOTH aarch64 boards, mounting `/` unconditionally (`:5409`). **The change reaches the Pi
whether or not it is aimed there — expect pi's bare-metal boot outcome to change, correctly.**
⚠ **THE LESSON, and it is not today's population rule:** I verified the peer's claim correctly and then
proposed a remedy that contradicted the project's direction. **Verification does not license design.**

### 2026-09-08 · **PETER: "a big diff with the pi vs orin is the microsd is simple to plug and pull on the pi"** — REVISES PI'S CARD-WRITER PLAN
Three consequences, and the first inverts an assumption:
1. ⚠ **THE EASY PULL IS WHAT CREATED THE HAZARD, NOT A MITIGATION.** A pulled Pi card goes into the host
   reader — **the same `/dev/mmcblk0` every other board's card lands in**, which `flash-pi4.sh` hardcodes
   and never interrogates. **That is the mechanism by which the Orin's card became a repurposed Pi card.**
   ⇒ **pi needs target identity MORE than orin does, precisely because its media moves.**
2. ⭐ **REVISES "promote `load-card10.sh`" — TAKE ITS REFUSAL MECHANISM, NOT ITS WRITE MECHANISM.** Its
   `udisksctl mount` + `cp -R` + pre-write harvest exists because the **Orin's card is awkward to pull**
   and must be edited where it sits. **The Pi can take a whole fresh image.** pi wants full-image `dd`
   + an identity check; the in-place machinery answers a question pi does not have.
3. ⭐ **BOOT-COLD IS LITERALLY TRUE ON PI, NOT A DISCIPLINE.** Peter's cold-boot rule says presume nothing
   "even tho we keep booting the same machine." On the Orin that is at least true of the storage. **On
   the Pi the drive is swappable in seconds — the card in the slot this boot may genuinely not be last
   boot's card. The kernel is not pretending not to know; it does not know.** Strongest available
   argument for the loader-serial root with no fallback, and it is a PI argument, not a jetson one.

### 2026-09-08 · ⭐ **GRANT #10 ISSUED TO orin 22 / BOOTROOT — re-key of `pi4-regression.spec` rows broken by boot-cold**
BOOTROOT lands the loader-serial root with no fallback; pi's kernel8 BootInfo carries
`boot_volume_serial: 0` (`arch/aarch64/boot.rs:1169`, orin's read at their tip `98213b7f`), so serial 0
→ `[vfs] root -> NONE reason=loader-named-no-volume`, **empty table**, and every spec row assuming `/`
is mounted reds. orin offered (a) grant them a one-commit re-key, or (b) pi takes the rows later and
their DONE gate records `kernel8-test` red-by-direction.

**CHOSE (a). Reason, one line: pi has no focus turn scheduled, and a gate everyone knows is red is a
gate everyone stops reading** — that costs more than the coupling. (This is the pi-7 lesson in
[[a-check-that-cannot-fire]] about a red with no defect behind it, applied forward.)

**CONDITIONS ATTACHED (six):** (1) the list is **MEASURED from a real `kernel8-test` run** with command
+ verdict quoted, never derived by reading the spec — observe-first is the standing condition on every
grant this seat gives; (2) **RE-KEY ONLY, NO DELETIONS** — a deletion silently drops coverage and the
floor, and any row that can only be deleted **comes back to pi**; (3) **floor arithmetic stated in the
commit message** — today **120 = 118 REQUIRE + 2 COUNT**, from `parse_spec`; (4) ⭐ **ADD one REQUIRE
for `[vfs] root -> NONE`** — the arc deletes a presumption and must install the proof the new behaviour
happens, or the property leaves the gate; confirmed 0 hits in the spec today, so it is a clean add,
**floor 120 → 121**; (5) new patterns follow today's bounding rules — `\b`-bounded single field, no
key spanning two space-joined fields, **never a trailing space** (stripped by `parse_spec`, which
INVERTS the key); (6) list sent before committing.

**PI'S CROSS-CHECK, limit stated:** rows lexically naming `vfs|mount|root|native|/fat|/usb` = **3**
(1 REQUIRE, 2 FORBID). ⚠ **Keyword bound, NOT behavioural** — a row asserting a shell verb's output can
depend on `/` without naming it. **Told them it is not a ceiling; a much larger measured list is
interesting, not wrong.**
**OWED TO PI'S FOCUS TURN: the Pi's loader passing a real serial** — the day pi inherits the mechanism.

### 2026-09-08 · ✖✖ **PI 9 WRONG #5, AND THE WORST ONE — grant condition #4 WOULD HAVE CERTIFIED THE DEFECT**
Peter: *"WTF does it matter what method I choose to boot? You are assuming too much."*
**THE ASSUMPTION, mine and in BOOTROOT's brief: that the mechanism is "was I HANDED a serial by a
loader?"** That makes the BOOT METHOD load-bearing — UEFI hand-off works, VideoCore firmware loading
`kernel8.img` does not — and the Pi fails by name **solely because its boot path has nobody to do the
telling. That is a presumption about the machine, i.e. the thing being deleted.**
**The card is the drive. The kernel enumerates what is there and finds the volume it booted from.
It should not need to be told.**

**I was wrong twice and the second is far worse:**
1. I wrote *"the Pi failing by name is the rule working."* **It is not — it is the design punishing a
   boot method.**
2. ⛔ **I then asked orin to `ADD REQUIRE [vfs] root -> NONE`, which would have made "the Pi cannot
   find its root" a REQUIRED WITNESS in `pi4-regression.spec`** — **a GREEN GATE CERTIFYING THE DEFECT**,
   and a row a future seat would have to argue its way past in order to FIX the bug. **Withdrawn same
   turn. Floor stays 120, not 121.** Rest of the grant stands (re-key only, no deletions, measured list,
   floor stated, bounding rules, list before commit).

⭐⭐ **THE CLASS, and it is new to this ledger and worse than any (a)/(b) instance: a gate row that
ENSHRINES the presumption the arc exists to remove.** Not a check that cannot fire — a check that fires
correctly, forever, in defence of the wrong behaviour. **Every instance today was a check failing to
SEE something. This one would have made the gate an ARGUMENT AGAINST THE FIX.**
**Test to add to the grant checklist: before requiring a witness, ask whether it asserts a PROPERTY or a
LIMITATION. Requiring a limitation makes the gate the defect's advocate.**

**AND THE SEAT RULE I KEEP BREAKING TODAY: verification does not license design.** Told orin I will
check what they build and will not design it from this seat. Twice in one session is a pattern, not a slip.

### 2026-09-08 · ✅ **BOOTROOT v3 SHAPE — pi claim VERIFIED, floor stays 120. Condition 4 struck by orin.**
New mechanism (orin 22, after Peter struck the loader hand-off): the kernel enumerates the disks it has
drivers for; **the disk carrying THIS kernel is the drive, proven by CONTENT** (running-image window vs
candidate file bytes; ELF → executable PT_LOAD at file offset, flat → offset 0) — **not by name, serial,
board or boot method.** Then layout over that disk: `/boot` = the FAT the kernel was found in ·
`/` = that disk's UnaFS volume if present else `/boot`'s · `/apps` = `/boot` at APPS/. No knob, no
cross-disk fallback; nothing found → one `[vfs] root -> NONE reason=… disks=…` and an empty table.

**PI CLAIM VERIFIED at `059e04db`** (*"same card carries both, so `/` stays native"*):
`arroyo:6103-6113` stages a 4 MB UnaFS volume with the in-repo `tools/unafs` CLI and passes it to
`make-pi-img.sh … 64 "$UNAFS_K3_IMG"` → **MBR p2 type 0x7f**. **Enclosing function `kernel8()` (`:5154`),
staging at FUNCTION-BODY indentation — not inside a conditional** ⇒ same image for metal and QEMU, as
`:6098` says outright. **One Pi card = FAT p1 (`kernel8.img`) + UnaFS p2, always. Floor 120.**
⚠ Limits handed over with it: pi is **94 behind trunk**, and **I verified the BUILDER, not a BOOT** —
media layout bounded, runtime behaviour not. A red pi row from BOOTROOT is information, not a contradiction.

⚠ **PI-SPECIFIC RISK FLAGGED FOR THEIR MEASUREMENT (named as a pi property, NOT a design proposal):**
the VideoCore firmware loads `kernel8.img` **flat** and jumps in; **the running image is then mutated
before any VFS work** — BSS zeroed, early boot writing into its own image region. **So the compare
window's placement is load-bearing on pi in a way it is not on a UEFI board, and it fails SILENTLY by
simply not matching: pi's disk becomes unidentifiable and you get `root -> NONE` for a reason having
nothing to do with the card.** Asked them to print window offset/length + compare result on the pi leg
so a miss is legible rather than a bare NONE.
**OWED TO PI'S FOCUS TURN: re-derive all of the above at pi's tip after the fold — this was checked 94
commits back.**

### 2026-09-08 · ⭐ **NEW PI FOCUS ITEM — the `/usb` WRITE path is a verified capability with ZERO gate coverage**
Found verifying orin 22's follow-up shape (non-root disks mount read-only at `/usb`/`/sd`). Their claim
was *"`/usb` exactly as today … same read-only posture."* **FALSE at `059e04db`:**
- `fs/vfs.rs:441` — *"BOT WRITE(10) path, and `FatBackend::read_only` reports **false for the `Usb`**"*
- `fs/vfs.rs:1493` — *"`read_only()` **false** — the `Usb` source carries a **verified** BOT WRITE(10) path"*
⚠ **THE TRAP: `world_readable`.** `new_usb` sets `world_readable: true` (`:484`) and it LOOKS like write
protection. It is not — the code says twice it is a **READ posture** (`:732`, `:1364`); the write gate is
`read_only()` (`:511`, enforced `:727`). ⇒ **"mount non-root disks ro" is a BEHAVIOUR CHANGE on pi.**

**Their "zero Pi row movement" prediction SURVIVES — for a reason they did not give.** The only pi spec
rows naming these volumes are three, all K3-mount (`:392` REQUIRE, `:1962`/`:1963` FORBID — the UnaFS
byte-verify). **NO pi row exercises `/usb`.** ⛔ **So a green PI-ROWS.md here means "the gate never
looked", NOT "the posture was preserved."** Told them not to read it the other way.

⭐⭐ **THE ITEM: witness the `/usb` write posture.** A capability that is live, deliberate, and called
"verified" in its own comment, **with zero coverage on either board** — this arc would have deleted it
silently and every gate would have stayed green. **An unwitnessed capability is the easiest thing to
delete by accident.** Sibling of the S13/`[u7stk]` shape (an unreached launcher is an unmeasured one)
and the exact inverse of the condition-4 error: **that one would have required a LIMITATION; this one
fails to require a PROPERTY.** Both are the same axis — ask what the gate asserts, and about what.
⚠ Re-derive after the fold: checked 94 commits back.

### 2026-09-08 · ⚠ **`/boot`'s `rw=` IS RUNTIME STATE, NOT A VOLUME PROPERTY — constrains pi's own witness work**
orin 22 corrected the read-only design on pi's finding (good: non-root disks now mount with their
SOURCE's posture, `/usb` stays writable) but relayed the veto map as *"Default/Usb → None"*.
**`Usb → None` confirmed. `Default` is CONDITIONAL** (`fs/fat.rs:684-698` @ `059e04db`):
```
Default => if drivers::block::default_writable() { None } else { Some(DEFAULT_VETO) }   // USBFALL F1 / FRGUARD
Usb     => None                                                                          // USB-WRITE F3
```
Its own comment: *"the global slot is refused in exactly one state — the boot volume positively found on
the OTHER handle; `unknown` and `unproven` fail OPEN."*
⇒ **`/boot` on pi is `BlockSource::Default`** (`vfs.rs:470`), so **the same card can mount `rw=yes` one
boot and `rw=no` the next.** Intended (FRGUARD), not a bug.

⛔ **CONSTRAINT ON PI'S OWN `/usb` WITNESS ITEM (previous entry): do NOT write a row requiring a fixed
`rw=` on a `Default`-sourced volume.** `/usb` (`Usb → None`, unconditional) is safe to witness with a
fixed expectation; **`/boot` is not — such a row passes on the common path and reds on the FRGUARD path.
A flaky row is the failure mode that teaches a seat to stop reading gate output.**
Hazard also handed to orin for their `rw == !read_only()` mutation check: for `Default` both sides must
be sampled at the SAME INSTANT, or the check is green on the common path and intermittent on the other.
⚠ Re-derive after the fold; `default_writable()` may have moved in 94 commits.

**✅ CLOSED 2026-09-08:** orin 22 verified `Default` at their tip (`fat.rs:678-686` @ `98213b7f`)
and folded it as amendment 03 item 4': `rw=` comes from the SAME `read_only()` call that builds the
mount, the mutation asserts within one call only, and **no fixed `rw=` expectation on a Default-sourced
mount anywhere**. Hazard discharged on their side; **pi's own constraint above still stands for pi's
witness item.**

### 2026-09-08 · ✅ **PETER'S "CATALINA AS ALIEN ENEMY" RULE — pi VERIFIED CLEAN, with one open point**
Peter 17:35Z: *"like on the macbook if UnaOS saw catalina and immediately formatted the disk as an alien
enemy."* Three clauses: **home** = the disk with this kernel · **friend** = another UnaOS disk, mounted
and left alone · **stranger** = any other disk, never touched on the kernel's initiative.
orin asked pi to verify whether a boot-time formatter exists in pi's lane and whether it is knob-gated.

**ANSWER: a kernel-side Pi installer EXISTS (`crates/kernel/src/install/pi.rs`) and is gated three deep,
none of it in the default image.** Base `K8_FEATS="baremetal,skip_xhci"` (`arroyo:5210`); gates added only
under their own knobs (`:5888-5899`):
```
UNAOS_PIINSTALL         Gate 1  emmc2 microSD READ-ONLY census + announce, NO WRITE
UNAOS_PIINSTALL_ARM     Gate 2  non-destructive scratch write/verify/RESTORE ladder
UNAOS_PIINSTALL_CONFIRM Gate 3  DESTRUCTIVE (GPT -> FAT32 -> payload -> verify)
```
Cumulative in `Cargo.toml:1819-1821` — **Gate 3 unreachable without 1 and 2.** And the MODULE DECLARATION
is itself gated (`lib.rs:92` `#[cfg(any(installdemo, install_target, piinstall))] pub mod install;`), so
with no knob it is **compiled out, not merely unreached** — checked the declaration, which is the trap
([[search-the-thing-not-the-name]]). `arroyo:5886` also notes `kernel8-install` arms _CONFIRM against a
**BLANK scratch image, NEVER the battery fixture.**

⚠⚠ **OPEN, AND IT IS THE POINT THAT MATTERS — NEW PI FOCUS ITEM: the gating is COMPILE-TIME.** An image
built with `UNAOS_PIINSTALL_CONFIRM=1` carries the destructive path in the binary, and the operator's
consent then lives in a **build-time env var, not at the moment of destruction.** **Whether Gate 3's
"about-to-destroy line" is a genuine RUNTIME prompt or only an announcement before proceeding is
UNVERIFIED** — I read the feature plumbing, not `install/pi.rs`'s control flow. **That is what decides
whether a confirm-built image could touch a stranger disk.** Read it at a focus turn; do not report the
board clean until then.
**Handed to orin for their lane, gating unconfirmed:** `sdmmc_tegra.rs:1863` calls `UnaFS::format(dev,
size_mb)`; nearest `#[cfg]` above is `install_target` at `:1795`, **but "nearest cfg above" is a heuristic,
not proof of enclosure** — told them so rather than reporting it as gated.

**✅ CLOSED SAME SESSION — Gate 3 read, and it is NOT the forbidden shape.** `install_flow`
(`install/pi.rs:311`) is a **SELF-CLONE**: step 0 mounts **the card's OWN boot partition**
(`fs::fat::mount()`, *"the running system's own boot media"*) and snapshots the tree into memory
**before any destructive write**, because *"the seated card is both the source and the target."*
Then GPT → zero ESP metadata → FAT32 → mirror the buffered tree back. **In Peter's taxonomy that is
HOME, not a stranger — no path touches a foreign disk on the kernel's initiative.**
**Real pre-write refusal exists:** `tree.file_count == 0` → *"not a self to clone — refuse rather than
'PASS' a hollow card"* → `Err(BadArg)` **before `write_gpt`.** Refuse first, destroy second.
⚠ **Self-correction: `run()` IS on the boot path** (`main.rs:485`, `#[cfg(feature="piinstall")]`) with
**0 `shell.rs` references and no runtime prompt** — which pointed at "kernel formats a disk on its own
initiative." **The self-clone target is what resolves it.** Left open in the report rather than raised
as an alarm, then closed by reading the flow — the correct order, and the opposite of this morning.
⚠ **RESIDUAL (bench hygiene, not a rule breach, and it stands): consent is BUILD-TIME, the
ABOUT-TO-DESTROY line is a `serial_println!` not a prompt, and "home" = WHATEVER CARD IS SEATED AT BOOT.**
⇒ **a `_CONFIRM`-built image is a card-eater for any card seated in that Pi** — and all three boards'
cards meet at the same host reader, which is how the Orin's card became a Pi card. **Same residual
applies to the Orin's `install_target` image; handed to orin.**

### 2026-09-08 · ✅ **BOOTROOT DONE — pi predicted correctly, floor unmoved, GRANT #10 UNSPENT**
`exec-orin22-bootroot` (`b0536d83`, `b1885dbc`, `b12decbb`). `./arroyo kernel8-test 300` → MBENCH PASS,
**no `pi4-regression.spec` change — PI-GRANT NOT EXERCISED.** The Pi kernel found itself by CONTENT:
`[vfs] root = boot volume serial=0xf3d9b41a source=global match=/KERNEL8.IMG unafs=present matches=1
window_off=0x80000 window_len=4096 file_off=0x0 candidates=12 disks=global=present usb=absent … ::`
`/` native, `/boot` FAT, **no board branch anywhere.** ⭐ **pi's BSS point was honoured**: window is
`_start` in `.text`, zero relocations in the RE segment — offset/len/candidates all on the wire, so a
future miss is legible rather than a bare NONE. **The boot proved it; pi had only verified the builder.**

⚠ **125 IS NOT PI'S FLOOR.** Their run reads `125/125`; **pi's floor at pi's tip re-derives to 120**
(`parse_spec`, this session). The gap is the 94 commits pi lacks — their `pi4-regression.spec` carries
the fold's directives. **Both right for their own tree.** The baton's standing warning is exactly this:
*"hw-jetson's is a different number, never quote another tree's."* **pi's fold lands at ~126, not 125.**
Told them not to record 125 as pi's number — a future pi seat would hunt a regression that is not there.

⭐ **DEVIATION THEY FLAGGED, VERIFIED PRESENT AT PI'S OWN SHA (not fold-introduced):** `fs/unafs.rs:520-522`
— *"UNAFSBIND: the entry is a `BoundMount`, and the lazy bind is `bind_mount` — handle-DISCOVERING, not
`Global`-assuming."* ⇒ **`/` is native IFF the discovered handle IS the matched disk's.** That makes pi's
"one card, `/` stays native" true **by discovery rather than by assumption** — better footing than the
claim pi originally gave them, and it holds today, not just after the fold.

**GRANT #10 REMAINS OPEN AND UNSPENT.** FOLLOWUP (KEEP13 merge + indexed bus mounts, source's own posture,
`rw=` sampled in one call) may still move a pi row — **the same five conditions apply unchanged; they do
not need to re-ask.**

### 2026-09-08 · **GATE BUNDLING — relayed ruling (orin 22's GIST of Peter 20:20Z, NOT verbatim)**
Per executor commit: only the TARGETED gate (touched arch's `check`, the one QEMU leg exercising the
change, its own RED-first mutation). FULL battery ONCE on the integrated tip as the arc's DONE gate.
Nothing for a change that cannot affect it. **For pi: `kernel8-test 300` stops being a per-commit cost
and becomes a per-arc one.** Their bulletin §14; LAWS §Gates at landing.
⚠ **Recorded as a RELAYED GIST — the wording is orin's, not Peter's. Re-read LAWS §Gates when it lands
rather than working from this paragraph** ([[relaying-upgrades-claims]]).
**Applies to pi's focus turns. No objection: BOOTROOT moved no pi row, so a per-commit battery today
would have been pure cost — pi's own evidence supports the ruling.**

⭐ **CLAUSE PI SENT BACK FOR LAWS, and today supplied the counterexample free of charge:**
*"nothing for a change that cannot affect it"* **is a POPULATION claim — the exact question this fleet
got wrong all day.** The failure is not skipping a gate that would have gone red. It is that
**"the gate cannot be AFFECTED by this change" and "the gate cannot SEE this change" are different
sentences, and only the first licenses a skip.**
**The instance, from pi's own lane:** orin's non-root read-only proposal would have flipped `/usb` from
writable to read-only on pi. **Pi's spec did not move — because NO pi row exercises `/usb` at all.**
Blind, not unaffected. Reasoning *"cannot affect the battery, skip"* would have been **right about the
outcome and wrong about the reason**, and the next such change deletes a verified BOT WRITE(10) path
with every gate green.
**Proposed wording:** *skip a gate only when the change cannot affect what the gate ASSERTS — and name
the assertion. "The gate has no row for this" is a reason to ADD one, never a reason to skip.*
**Corollary, free:** a skip that NAMES the assertion it deemed unaffected is auditable; a bare
"not applicable" is not. Same discipline as claims carrying their command.

**✅ ADOPTED VERBATIM into LAWS §Gates (orin 22 bulletin §14): skip only when the change cannot affect
what the gate ASSERTS · NAME the assertion · "no row for this" ADDS a row rather than skipping. The
`/usb` blindness is the cited instance. Executors under the rule write the named assertion in the commit
body — so a skip is auditable from `git log` alone.** Rule is LIVE, not proposed.

### 2026-09-08 · ⭐⭐ **THE ADJACENCY CLASS SPLITS IN TWO — rmbp 16's contract find, applied to pi's three rows**
rmbp opened a direct channel (R2: peers are live; a day of latency was spent relaying through orin) and
handed over the reframe: **`video/wcg.rs` DOCUMENTS a field-insertion contract**, verified here at
**`:4066-4072`** (⚠ **rmbp has it at `:3920-3925` — line drift; neither tree cites the other's numbers**):
> *"the pi4 gate matches `\[wc-g\] rollup win=.* scope=window .*frame_us=.* ->`, so **fields may be
> inserted between matched keys** and nothing may be renamed, reordered, or moved past the terminal
> verdict."* — and it quotes the gate's own pattern, which makes it a SPECIFICATION, not a note.
⇒ **rmbp's class: a rule written STRICTER than the contract its emitter is maintained to.** The
insertion that killed the aarch64 FORBID was **LEGAL under this tree's own stated contract.**

⛔ **BUT APPLYING IT TO PI'S THREE ROWS SPLITS THEM, AND THE REMEDY DIFFERS:**
- **`:1132` `[wc-h]`** — compositor family, but the contract names **`[wc-g] rollup` specifically**; no
  contract found naming `[wc-h]`. Unresolved, leaning uncovered.
- **`:1484` + `:1549` `[pstrip]`** — emitter `ui_status.rs`, **NO CONTRACT AT ALL.** Its comments discuss
  the gate (`:109-110`, `:150`) but never state what may be inserted/renamed/reordered. **Control: the
  same grep style found wcg.rs's, so the absence is about the file, not the pattern.**

⭐ **TWO CLASSES, NOT ONE. rmbp's = rule stricter than a STATED contract. Pi's = rule with NO contract**,
where the gate's expectations are unstated and the emitter's author cannot learn them. **Re-keying fixes
neither properly; for the second the remedy is to WRITE THE CONTRACT AT THE EMITTER** — the pi-7 lesson
already in [[a-check-that-cannot-fire]]: *an undocumented invariant is load-bearing and deletable at once;
the remedy is not a norm and not a gate change but a contract.*

⛔ **`:1549` IS THE ACUTE CASE AND THE REMEDY IS NOW BIGGER THAN "widen the junctions".** Its spec comment
says emitter-side `paced=yes/no` *"remains the real fix and is still owed"* — a future edit to
`ui_status.rs` — and **nothing at that emitter tells its author a FORBID depends on four of its fields
staying adjacent.** Whoever lands `paced=` will be doing what `[wc-g]`'s contract explicitly PERMITS, and
on `[pstrip]` it silently greens the guard. **FOCUS ACTION, revised: write the `[pstrip]` contract at
`ui_status.rs` FIRST, then re-key `:1484`/`:1549` to match it.** Pi owns both.
**Settled across three trees:** `mbench.py` `.strip()` — pi `059e04db`, orin `98213b7f:248`, rmbp `:256`.

### 2026-09-08 · ⛔⛔ **`\b` IS NOT THE GENERAL BOUND — the fleet standardised on it this morning and it is INERT on hyphenated siblings**
rmbp 16 flagged a trap: `wcg.rs:1628` (mine; theirs `:1495-1503`) documents that **the pi4 spec relies on
`scope=window ` carrying a TRAILING SPACE so it cannot match `scope=window-band`** — while `mbench.py`
**strips every spec line.** Safe today only because the space is INTERIOR. **I executed it, and it is worse
than stated.** Wire: `[wc-h] rollup win=1 scope=window-band emit=6 declines=0 -> TEAR-FREE`
```
scope=window            (trailing space)  -> exit 1  FALSE HIT   (parser strips it)
scope=window\b                            -> exit 1  FALSE HIT   ⛔ \b DOES NOT DISCRIMINATE
scope=window(?=\s|$)                      -> exit 0  correct
scope=window .*declines=  (pi's TODAY)    -> exit 0  correct — pi is SAFE today
```
⭐ **WHY: after `window` comes `-`, a NON-word character, so `\b` is satisfied right there.** orin's
`span_blocks=2048\b` is sound because `20480` extends with `0`, a WORD char. **Both true; only one
generalises — and their framing ("`\b` is the only bounded form; `(?=\s|$)` if you need it without `\b`
semantics") makes the inert one the DEFAULT.** Sent to orin as a rule-text correction (NOT a re-open of
`jetson-sync1.spec:543`, which is fine) and to rmbp.

**THE RULE: choose the bound by what the SIBLING TOKEN STARTS WITH.** `\b` guards only against
WORD-character extension. **Non-word sibling (hyphenated/punctuated) ⇒ `\b` inert, `(?=\s|$)` mandatory.**
Never a trailing space either way. **And: a new bound must ship with a negative control that MATCHES the
sibling it must exclude** — same shape as `.strip()`, invisible by reading, only a control fires it.

⚠ **PI'S LIVE EXPOSURE, and the hazard is TIGHTENING, not editing:** `:951 [wc-g]` and `:1023 [wc-h]` both
use the interior-space form and **discriminate correctly today.** The moment either is re-keyed to END at
`scope=window `, the parser strips it and it starts matching `window-band` — which `:1025`'s own comment
confirms is a real second rollup sharing counters. **Named hazard for the contract work.**

**CORRECTION TO PI'S PREVIOUS ENTRY: `[wc-h]` IS class 1, not "leaning uncovered."** Contract confirmed
here — *"KEY ORDER IS LOAD-BEARING ACROSS SEATS"* (`wcg.rs:1626`), naming `win=`/`scope=`/`declines=`/
terminal verdict as matched **by the pi4 track's spec**, plus every insertion since. **A contract written
in rmbp's file FOR pi's gate.** ⇒ **pi's class-2 rows are `[pstrip]` ONLY.**
**Contract form settled with rmbp: use `wcg.rs:1626`'s shape for `[pstrip]`** — names the consuming spec,
the load-bearing key order, the terminal rule, and every insertion made since. **One shape across lanes,
so a gate over contracts stays possible later.**

**✅ RULE ADOPTED + FIRST APPLICATION VERIFIED (2026-09-08).** orin took the bounding rule verbatim
into LAWS and as a standing line in every later brief. Pi executed its first two applications from an
independent tree: `\bmatches=1\b` and `\bwindow_len=4096\b` each **exclude their sibling (exit 1) AND
match the real wire (exit 0)** — word-char siblings, `\b` correct, rows stand. ⭐ **TWO WIRES EACH IS THE
STANDARD FORM: a bound that only excludes could be excluding everything.** Provenance: trap rmbp 16, `\b`
failure pi 9 by execution, wording pi 9.

### 2026-09-08 · ⭐ **"A RULE OF THUMB TRAVELS FASTER THAN ITS TEST" — rmbp 16, and it is self-demonstrating**
rmbp re-executed the boundary table independently (reproduces exactly), **then audited their own artifacts
and found the WRONG guidance had already spread**: their B85 row and focus queue both recommended `\b` as
the default, inherited from the earlier LAWS prose, **never tested against a hyphenated sibling.**
⇒ **The bad rule reached a second lane's artifacts before anyone executed it.** Prose is quotable; a test
is not, so the prose form is what propagates.
**FIX ADOPTED AND RELAYED TO ORIN: the bounding rule ships WITH the four-row execution table, not as
prose** — a reader who quotes the table cannot extract "use `\b`" from it; a reader who quoted the prose
already did, twice. ⭐ **Self-demonstrating: the earlier wording was a claim about a CLASS (`\b` bounds
keys) verified on an INSTANCE (`2048`/`20480`) — the day's own error, inside the rule written to stop it.**
**Provenance: trap rmbp · `\b` failure pi by execution · spread-detection rmbp (self-audit, unprompted).**

**PI SELF-CHECK, same class, clean:** this queue's only 4-column table escapes its regex alternation
(`([2-9]\|[0-9]{2,})` — 5 structural pipes + 1 escaped). ⚠ **My verifying check was itself defective** —
it assumed 5 pipes meant well-formed and so flagged every 3-COLUMN table in the file as debris. Result
kept only because check 1 (pipe/escape counts on the actual table) was sound. **A check built to find the
day's defect containing the day's defect; noting it rather than reporting its output as findings.**
⚠ **CARRY INTO THE `[pstrip]` CONTRACT WORK: rmbp has had THREE stray-pipe injections into one
GATE-LEDGER row today, every one from a regex, in a row whose subject is regexes** — debris past the last
validated column passes silently. **If pi's contract work adds a ledger row quoting a regex, escape the
pipe.** The ledger-cell pipe convention is Peter's call and already on his list — **do not duplicate the
ask; the corroboration is recorded here.**

**✅ CLOSED:** orin replaced the prose rule with the four-row table (both wires) in the bulletin and the
standing brief file, headed **"quote the table, never a summary of it"**; the prose form is deleted from
both. Provenance recorded, spread-detection to rmbp 16. **The fix now propagates with its own falsifier
attached — which is what the rest of the day’s rules ask for anyway.**

### 2026-09-08 · **FASTK8 RULING (relayed gist, Peter 21:15Z) + TWO PI REFINEMENTS — one corrects pi's own claim**
Ordinary `kernel8-test` runs exit at the LAST COMPLETE marker + a grace window (default **20 s**; the
three phase-unbound builtins and every FORBID stay live through it); **the arc's DONE gate keeps the full
300 s wall.** Opt-in `UNAOS_K8_FAST=1`, `arroyo`/`mbench` only, no spec change. Milestones: m1 a FORBID
inside grace reds the fast run · **m2 a FORBID beyond grace is HIDDEN BY DESIGN and demonstrated once** ·
m3 no marker → full wall. Pi's line-position measurement is the cited evidence. Reaches pi at trunk sync.

**✅ REFINEMENT 1 — the 20 s default IS adequate for the thing pi flagged.** `[shellup]`, the ONE tag
appearing only in the discarded window, carries a clock: `census t=12908ms`. The fast exit (line 2171)
precedes it, so a 20 s grace contains it.

⚠ **REFINEMENT 2 — CORRECTS PI'S OWN EARLIER CLAIM. `[u7stk]` is NOT lost by fast mode; its VALUE is.**
Pi told orin *"163 land in the discarded window, so a follow exit takes that instrument away."* **163 is
right; the framing was wrong.** Measured: **219 total, FIRST AT LINE 197 — before the exit** — so 56
samples land inside fast mode and the instrument still fires.
⭐ **But the line carries `used=272 hw=272 headroom=32496`, and `hw=` is a HIGH-WATER MARK.** Owed item #4
(*is `U7_LAUNCH_STACK_SIZE = 32 KiB` still right?*) is answered by the **MAXIMUM `hw` over the run**, which
grows monotonically. **Fast mode keeps the early samples where `hw` is smallest and discards the late ones
where it is largest.** ⇒ **A fast-mode headroom reading is a FLOOR, not the measurement — true,
reassuring, and wrong.** Same family as the condition-4 error: not a check that fails to fire, but one
reporting honestly about a window that excludes the answer.
**ASKED OF FASTK8 (one line, not a design change): say `fast=1 grace=20s` ON THE WIRE at exit**, so a
later reader of any monotonic accumulator can see the capture was truncated by policy. Without it a fast
capture and a full capture are indistinguishable after the fact.
⛔ **PI RULE, RECORDED: owed item #4 MUST be measured on a FULL-WALL run. Fast mode is sound for
pass/fail; it is NOT sound for any monotonic accumulator's final value.**

### 2026-09-08 · ⚠ **THE FASTK8 TRAILER IS A NEW LINE IN EVERY SPEC'S POPULATION — checked pi, flagged fleet-wide**
orin's trailer (better than pi asked: **BOTH modes stamped**, so an unstamped log is unambiguous too) goes
**IN the captured log**, therefore `--replay` feeds it to every directive of every spec on every board.
⛔ **THE HAZARD: the trailer contains the literal word `COMPLETE`** (*"last COMPLETE at +N.Ns"*), and
COMPLETE directives are what decide the **TRUNCATED** verdict (`Matcher.truncated()`; `run_verdict` rule 2).
**A spec with a loose COMPLETE pattern would have its end-of-run marker satisfied by the trailer on EVERY
run — permanently disabling truncation detection, silently, reading PASS where the truth is TRUNCATED.**
That is the one verdict whose entire job is to say *"this run proves nothing."*

✅ **PI CLEAN, BY EXECUTION not inspection** — both trailer forms fed through `mbench.Matcher`:
`pi4-regression.spec` (260 directives) and `pi4-barename.spec` (9) → **no directive matches, either form.**
`DEFAULT_FORBIDS` safe: the trailer says `FAST`, not `FAIL`.
**Sent orin the three-line check to run against `jetson-sync1.spec` + rmbp's seven x86 specs before FASTK8
lands**, with the remedy named: **if any spec hits, fix the TRAILER's wording, never the spec** — a
diagnostic line must never be able to satisfy a gate directive (cheapest: drop the bare word `COMPLETE`).

⭐ **SAME LESSON AS `\b`, DIFFERENT COSTUME: the trailer is CORRECT. The failure would be a correct new
line landing inside a population nobody re-checked. ANYTHING ADDED TO THE WIRE JOINS EVERY SPEC'S INPUT.**

### 2026-09-08 · ✖✖ **PI 9 WRONG #6 — I PROPOSED A HARDCODE, INSIDE THE THREAD ABOUT NOT HARDCODING**
Peter called it. **My line:** *"cheapest wording fix if one collides: avoid the bare word `COMPLETE` in
the trailer."* ⛔ **That makes GATE CORRECTNESS DEPEND ON A CHOSEN LITERAL.** It holds only until someone
rewords the trailer, translates it, or adds a spec pattern loose enough to catch the new wording — **and
nothing detects any of those.** A magic string standing in for a structural property.

⭐ **THE STRUCTURAL FACT I WALKED PAST: the trailer is HARNESS output, not KERNEL output.**
`arroyo`/`mbench` writes it; the kernel never emits it. **The harness knows exactly which lines it wrote,
so those lines should not be in the matcher's population at all — exclude BY CONSTRUCTION, not by
phrasing.** Then the wording is free, no spec can be affected, **and my whole "run this check against
every spec on every board" errand disappears — an errand that existed only because the boundary was not
drawn.**
**Retracted to orin the same turn. The CHECK stands (pi verified clean by execution, both specs, both
trailer forms); the REMEDY is withdrawn. If a collision appears, the answer is the boundary, never the
vocabulary.**

⚠ **THE TENSION TO RESOLVE, stated as a property and left to their arc:** *"stamp it in the log so the
captures are distinguishable"* and *"the log is the matcher's input"* are in conflict. **The conflict is
the thing to fix — not the one word that happens to collide today.**

⭐⭐ **THE PATTERN IN MY OWN ERRORS TODAY, now six: I verify correctly, then propose a remedy that
contradicts the project's direction** (condition #4 required a limitation; the `serial==0` guard was a
knob; this was a magic string). **Verification does not license design — third time. The rule is not
"be careful"; it is: NAME THE PROPERTY, LET THE OWNING SEAT PICK THE MECHANISM.**

### 2026-09-08 · **TRAILER SWEEP 11/11 CLEAN — but the safety still RESIDES in the sweep, and that decays**
orin ran pi's check across all 11 specs × 3 trailer forms → `feed_raw` = `[]` on every one
(pi4-regression **266** directives, pi4-barename 9, jetson-sync1 134, jetson-jd5 19, rmbp-boot 23,
round6-rmbp 60, x86-fat 69, x86-holocron 18, x86-wc 11, x86-wifival 31, x86-witness 87). **Real baseline,
worth keeping.** ⚠ Note pi4-regression is **266 at their tip vs 260 at pi's** — the fold again; never
quote across trees.
⭐ **PETER'S GENERAL-FORM RULING: *"wtf is kernel8, why is there an exception"*** — the executor was
respawned as ONE helper for EVERY QEMU verb, default fast, `UNAOS_QEMU_FULL=1` for the DONE gate.
**Same ruling that killed pi's `serial == 0` guard: no per-board exceptions.**

⛔ **BUT THE HARDCODE SURVIVED IN THEIR REPLY** (my retraction crossed their run): *"the wording never
uses the bare word COMPLETE ('completion'); the check re-runs on the executor's final text before it
lands."* **Both halves are the thing withdrawn:** the wording makes correctness depend on a chosen
literal; **the re-run-before-landing is a mitigation that decays the moment it succeeds** — it holds at
landing, not after. A later reword, or a later loose spec pattern, breaks it with every gate green and
nobody re-running an 11-spec sweep.
⭐ **AND IT IS THE ARGUMENT ORIN AND I ALREADY SETTLED TODAY, REVERSED.** They proposed *"shown again
after every sibling change"*; pi said a recurring obligation is true, agreed, and quietly not done six
weeks later; **they struck it for the structural form.** This is that, wearing the harness's clothes.
⚠⚠ **THE DECAY CLOCK IS IN THEIR OWN MESSAGE: *"only `pi4-regression` declares COMPLETE markers today."***
The collision surface is ONE spec now — **and fast mode exists to make declaring COMPLETE markers
attractive. THE HAZARD GROWS EXACTLY AS THE FEATURE SUCCEEDS**, while the sweep proving it clean gets
longer and less likely to be re-run.
**Property named, mechanism left to them** (pi has over-reached 3× today): *harness-written lines must be
outside the matcher's population BY CONSTRUCTION, not by vocabulary — the harness knows which lines it
wrote, and throws that knowledge away before matching.*

### 2026-09-08 · ✅ **BOUNDARY DRAWN STRUCTURALLY — no harness text in the log at all**
orin's resolution (their mechanism, pi's property): **the serial log stays PURE GUEST BYTES** — anchored on
the rule the tree already has, *"the guest's bytes still land in exactly `$logf` and mbench replays exactly
that file"* — and the harness writes run metadata to a **sidecar `<log>.run`** (mode, completion time,
grace, wall, cap, spec, sha); mbench's verdict line reads the sidecar and prints the mode. **No harness text
can reach a directive; no vocabulary check is load-bearing; the 11-spec sweep demotes to "a fact about
today, not a control."** The *"check re-runs before landing"* sentence is withdrawn with it.
⭐ **Drawn better than pi named it: a RESTORATION of an existing rule, not a new invention.**

⚠ **ONE PROPERTY THE SIDECAR INHERITS — and `mbench` already learned it once.** `run_verdict`'s own
docstring: *"This REPLACED a `passed()` boolean. Two-valued was the shape of the problem: with only
pass/not-pass there was nowhere to put 'the capture stopped early', so a short log had to be reported as
one of the two things it was not."*
**The sidecar recreates that fork: reading `<log>.run` has THREE outcomes — says FAST · says FULL ·
ABSENT/UNREADABLE/STALE** (a log copied without its sidecar; a capture predating this; a re-used `$logf`
whose sidecar was not rewritten). **Collapsing the third into either of the first two makes the verdict
line assert a mode nobody measured — the exact defect `run_verdict` was rewritten to remove, one layer out.**
⚠ **It also costs what was good about the in-log trailer: stamping BOTH modes made ABSENCE meaningful.**
A sidecar's absence is ambiguous unless the third state is carried explicitly.
⚠ **Staleness is the sharp edge, not absence: an absent file is obvious, a STALE one is confidently wrong**
— pi 7's asymmetry (*wrong-lenient sends you looking; wrong-strict stops you looking*).
**Property named, mechanism theirs. Recorded in their bulletin as pi's property / orin's mechanism.**

**✅ SIDECAR MECHANISM SETTLED:** three-valued read (fast / full / **unknown**, printed in the verdict
line), and **staleness made DETECTABLE rather than trusted** — the sidecar carries the log's identity
(size + sha256 at write time); an identity that does not match the log beside it reads **`stale`, never a
mode**. Cited in their brief as the `run_verdict` lesson one layer out.
⚠ **ORDERING PROPERTY PI FLAGGED (mechanism theirs): the identity must be computed over the log AS THE
VERDICT WILL READ IT, not as it stood when the MODE was decided.** The natural write point is the exit
decision — that is when mode/grace/completion are known — **but any byte landing afterwards (late flush,
QEMU teardown, trailing newline) mismatches, and then EVERY run reads `stale`.** A systematic misfire, and
**that is exactly how a protection gets switched off** — the same argument as the card-identity refusal.
⭐ **Direction is right and worth keeping: a false `stale` is LOUD and sends you looking; a false MODE is
quiet and stops you looking.** Failing toward `stale` is the correct asymmetry — the flag is only that
failing there *always* converts a good check into a deleted one.

**✅ CLOSED:** values decided at the exit decision, **FILE written last** — after QEMU exits and is
waited on, immediately before replay — so the identity covers the log as the verdict reads it. ⭐ **Proved
BOTH directions, unprompted: one byte appended after the sidecar → `stale` (fires); three consecutive
normal runs → mode read every time (does not over-fire).** That is the two-wire standard from the bounding
work, applied by orin to a different mechanism without being asked — **a bound that only excludes could be
excluding everything.**

### 2026-09-08 · ⛔ **CONCURRENCY RULING (Peter 23:25Z, VERBATIM relay) MAKES FLAKE-1's CONDITION ROUTINE**
Peter: *"FOR SURE I DO NOT WANT TO WAIT FOR THE OTHER PLATFORM'S TESTS TO WAIT FOR THE MORE DEFINITIVE
METAL BOOT. FROM NOW ON FOCUS PLATFORM ONLY UNTIL METAL RESULTS ARE IN SO THE OTHER ARCH'S TESTING IS DONE
WHILE WE MOVE FORWARD TO THE NEXT ROUND."* ⇒ during an Orin week pi's `kernel8-test` runs in the
background while the Orin card is written and flown; **its result is a LANDING condition, never a FLIGHT
condition.** Bulletin §18; LAWS §Gates at landing. **Split is right.**

⛔ **BUT THE RULING INSTITUTIONALISES PI'S WORST DOCUMENTED FLAKE.** `arroyo:6153-6159`, verbatim:
*"two flake modes … both produced a DISHONEST verdict from a harness whose whole point is honesty.
(1) SILENT NO-CAPTURE. A `kernel8-test 150` finished looking green while QEMU had produced NO
serial-pi.log at all; mbench then errored `[Errno 2]` and a downstream `&&`/pipe masked the failure.
**Root cause is a CHECK-THEN-BIND RACE on the QMP port** … **a concurrently-launching QEMU from another
worktree gate**"* — **and the documented failure is a FALSE GREEN, not a truncation.**

**TWO PROPERTIES SENT FOR LAWS §Gates:**
**(a) A background run that FINISHED is not a run that produced a VERDICT.** The tree already separates
them: retry-exhausted → **exit 4** (*"carries NO verdict: not a pass, not a regression, not a
truncation"*), truncation → **exit 3**. **A landing condition must require rc 0, never "the background job
completed."** ⭐ **Third instance today of the same three-valued lesson: `passed()` → `run_verdict`, the
sidecar mode, and now this.**
**(b) The mitigation is IN-TREE and belongs in the rule, not rediscovered:** the pre-scan is *"a NARROWING
measure, not a guarantee"*; **pin `UNAOS_QMP_PORT`** — concurrent runs must not share the default (`4463`,
`4464` typed). **If concurrency is policy, distinct ports are policy, not an operator's good habit.**
⚠ **Second-order, and it cuts against the comfort:** fast mode SHORTENS pi's racing window — but **the
DONE-gate FULL WALL is the landing condition, so the long contended run is exactly the one that matters.
The run that must be reliable is the one the ruling puts under load.**

**✅ ADOPTED (FLAKE-1 text verified surviving the fold in orin's tree):** two clauses beside the ruling in
LAWS §Gates — **(a) a landing condition requires a VERDICT (rc 0), never "the background job finished";
rc 3 / rc 4 = no evidence yet.** **(b) concurrent QEMU runs pin distinct `UNAOS_QMP_PORT`s as POLICY.**
Their background-battery runner sets a per-worktree port and records rc per leg; **the landing checklist
reads rcs, not completion.** Applies to pi at trunk sync.

⛔ **CORRECTION TO THE ABOVE RELAY (Peter: "NO I SAID UNTIL METAL TEST RESULTS ARE IN").** The entry above
records the wrong version — **`kernel8-test` does NOT run during the Orin flight, background or
otherwise.** It starts **only after the metal result is in**, concurrent with the NEXT round's work.
Left uncorrected in place per append-never-rewrite; **read this clause, not that sentence.**
⭐ **The corrected rule is STRICTLY BETTER for the hazard pi raised:** the flight gets a QUIET HOST, so
the definitive metal boot is not racing a QEMU gate from another worktree — **contention is removed from
the moment that matters most**, which is exactly what FLAKE-1's check-then-bind race threatens.
✅ **Pi's two clauses survive with their scope moved: they govern POST-METAL concurrency** — other-arch
batteries beside the next round's executors — **not the flight.** Still real multi-worktree QEMU
concurrency, so **(a) verdict ≠ finished (rc 0 required; rc 3/4 = no evidence) and (b) distinct
`UNAOS_QMP_PORT`s** both still apply, unchanged.
⚠ **Second relayed ruling today to arrive materially wrong and be corrected by the relayer** (the comms
clamp was the first; the gate-bundling arrived as an explicit gist). **The pattern is not carelessness —
it is that a ruling's SCOPE is the part that does not survive relay.** [[relaying-upgrades-claims]]:
carry the quote, and treat any un-quoted scope word as unverified.

### 2026-09-08 · ⛔ **LABEL MOUNTS (`/volumes/<LABEL>`, Peter struck bus-indexed points) REINSTATE THE VFS-4 BUG**
Ruling tonight: design closes first, brief frozen, ONE gate at the end; nothing of the other arch runs
until the Orin's metal result is in. All kernel8 proofs (finder mutations, fast-mode m1/m2/m3, sidecar
stale/3×) are **POST-METAL**. FOLLOWUP/QEMUFAST stopped; one executor **INTEGRATE** applies the frozen set
with only the Orin gate, then stages render11.

⛔ **PI FINDING SENT WHILE INTEGRATE IS LIVE — `shell.rs:5455`:**
```rust
const RESERVED_VOLUME_PREFIXES: &[&str] = &["/usb", "/fat"];
```
consumed by `unmounted_reserved_volume` (`:5464`); purpose written out at `:5440-5452` — a mutating verb
aimed at a reserved volume **not currently mounted** must say *"volume not mounted"*, **never fall through
to native root and mis-report a bare `-ENOENT`.** The comment records the P44 incident and says the
misdirection **"cost bench time."**
⇒ **Move mounts to `/volumes/<LABEL>` and this list matches NOTHING** → `unmounted_reserved_volume`
returns `None` for every path → **a write to an absent volume falls through to native root with a bare
`-ENOENT`: the exact regression VFS-4 was written to remove.**
⚠ **NOT FIXABLE BY EDITING THE STRINGS — that is why it is a finding.** The list is a **static enumeration
of spellings**; `/volumes/<LABEL>` is **discovered at runtime**. **No static list of labels exists to
write.** A hardcoded prefix table and a label-derived namespace are structurally incompatible — the same
direction Peter has pushed all evening.
**Property sent, mechanism theirs:** *"volume not mounted" must be derivable from the LIVE MOUNT SET, not
from a compile-time list of spellings* — and the live set is already the function's first argument
(`mounted: &[&str]`).

**PI SPEC CONSEQUENCE, RE-DERIVED EARLY: ZERO pi rows name `/usb` OR `/volumes`** (measured, both
spellings, all directive kinds). **Grant #10 stays unspent; no pi row moves.** ⚠ **THIRD TIME: that is
BLINDNESS, NOT SAFETY** — same `/usb` hole queued this afternoon. **When the post-metal run reports zero,
it must not be read as "the change was inert on pi."**

**✅ RESOLVED (orin's mechanism, pi's property):** **`/volumes` is the ONE static spelling** — the OS's
namespace root, like `/boot` — and *"volume not mounted"* for anything below it is **derived from the LIVE
mount set**: a path whose SECOND COMPONENT is not a current mount answers *"volume not mounted:
/volumes/<name>"*, never the native root's `-ENOENT`. `/usb` leaves the list, comment rewritten.
**RED-first fixture in VFS-4's own shape.** Going in as a seat edit before INTEGRATE's single gate, or as
the first follow-on if the gate has already run.
⭐ **Their scheme also improves on a hazard pi did not raise:** VFS-4's prefix matching needed a hand-written
boundary rule (*"`/usb` and `/usb/…` name the volume, but `/usbfoo` does NOT"*). **Component-wise matching
on the second path element makes that boundary structural instead of hand-checked.**
⚠ **TREE DIVERGENCE TO CARRY: their tip has `RESERVED_VOLUME_PREFIXES = &["/usb", "/boot"]`; pi has
`&["/usb", "/fat"]`.** The fold renames `/fat` → `/boot`. **Re-derive this entry after the fold — pi's
`/fat` spelling is 94 commits stale.**
**Recorded verbatim on their side as the reading of the post-metal list: "zero rows = blindness, not safety."**

### 2026-09-08 · ⭐ **PROVENANCE FOR FOCUS-FIRST (Peter 01:00Z, VERBATIM) — and it resolves a tension pi kept hitting**
> *"THIS WILL HELP WHEN WE START SELF HOSTING WHERE WE WILL BE REBOOTING THE VERY MACHINE WE JUST WROTE
> THE CODE ON. THAT'S WHEN THE HARD FOCUS COMES INTO PLAY. THE CODE IS STILL GENERALIZED BUT THE MACHINE
> ITSELF IS COMPILING AND TESTING **FIRST** THEN THE WIDER AUDIENCE GETS THEIR RUN."*

⭐⭐ **THE DISTINCTION PI HAD BEEN COLLAPSING: generalization is a property of the CODE; focus is a
property of the SCHEDULE.** *"No per-board exceptions"* (which killed pi's `serial == 0` guard and the
`kernel8`-only fast mode) and *"focus platform only"* are **not in tension — they live in different
spaces.** Every time today the two seemed to conflict, this was the resolution and pi did not have it.

⭐ **AND IT RECONTEXTUALISES TODAY'S INSTALLER AUDIT.** `install/pi.rs` Gate 3 — *snapshot the boot tree
into memory, lay a fresh GPT, mirror it back*, with the seated card **both source and target** — **IS
"rebooting the very machine we just wrote the code on."** Pi audited it as a safety question and cleared
it (home, not stranger). **Under the self-hosting frame it is not a bench oddity, it is the PRIMITIVE**,
and pi is the fleet's most self-hosting-ready path: an in-kernel installer that clones its own boot media.
⚠ **THEREFORE THE RESIDUAL PI RECORDED MATTERS MORE, NOT LESS: consent is BUILD-TIME, the
ABOUT-TO-DESTROY line is a `serial_println!` not a prompt, and "home" = whatever card is seated at boot.**
**A path meant to run ROUTINELY under self-hosting cannot keep destruction-time consent in a build-time
env var.** Re-read `install/pi.rs`'s control flow at a focus turn with this frame, not the audit frame.

### 2026-09-08 · ⭐ **SELF-HOSTING + HEALING (Peter 01:15Z, VERBATIM) — ties three of today's findings together**
> *"when we have self hosting with healing the machine will run itself until it runs right and we do not
> want all that great work hard coded as an appendage special case situation we want to grow UnaOS."*

**Three consequences for pi, all from findings already in this file:**

1. ⭐ **A HEALING LOOP NEEDS A THREE-VALUED VERDICT, NOT A BOOLEAN — and today hit that lesson four
   times.** *"Runs itself until it runs right"* consumes a verdict. **A loop that reads no-verdict
   (rc 4) or TRUNCATED (rc 3) as "not right yet" spins forever; one that reads it as "right" stops on
   no evidence.** `mbench` already learned it (`passed()` → `run_verdict`: *"with only pass/not-pass
   there was nowhere to put 'the capture stopped early'"*), then the sidecar mode, then the landing
   condition. **Healing is the consumer that makes it structural rather than stylistic.**

2. ⛔ **THE INSTALLER IS AN APPENDAGE BY CONSTRUCTION TODAY, WHICH IS THE SHAPE PETER NAMES.**
   `install/pi.rs` — the self-clone that IS the self-hosting primitive — is behind **three cumulative
   Cargo features** (`piinstall` ⇒ `_arm` ⇒ `_confirm`), the module declaration itself is
   `#[cfg(any(installdemo, install_target, piinstall))]`, and **none is in the default image**
   (`K8_FEATS="baremetal,skip_xhci"`). **The primitive is compiled OUT of the OS and switched on by
   env knobs at build time — an appendage special case, exactly.**
   ⚠ **NOT ASSERTING IT SHOULD BE UNGATED — the gating exists because the path is destructive, and that
   is a real reason.** Naming the tension: **safety-by-compile-time-absence vs. growing the primitive
   into the OS. Both cannot hold.**

3. ⭐ **AND THE TWO RESOLVE EACH OTHER, WHICH IS WHY THIS IS WORTH A FOCUS TURN.** The residual pi already
   recorded is *"consent is BUILD-TIME; the ABOUT-TO-DESTROY line is a `serial_println!`, not a prompt."*
   **Move consent to DESTRUCTION TIME and the compile-time gates stop being the safety mechanism — at
   which point the path can be first-class in-tree without being dangerous.** The knobs are standing in
   for a runtime decision that was never built. **That is the shape of the pi self-hosting item.**

### 2026-09-08 · ⭐ **ROADMAP (Peter 01:35Z, VERBATIM) — A/B KNOWN-GOOD BOOT DISK. PI'S OWN FIRMWARE FEATURE.**
> *"when in healing mode the machine will need an alternate boot disk with the last known good bootable
> version to help when there's a hard lockup of the test kernel."*
orin flags the VideoCore **tryboot / autoboot.txt** mechanism as the generic fallback **below** the kernel
(boot the test partition once; if the kernel does not clear the flag, the next boot takes the known-good).
⭐ **Right layer — a fallback that lives below the kernel is the only kind that survives the hard lockup it
exists for.** **Lands as the Pi's own firmware feature, NOT a bench script.** ROADMAP, not this arc.

**THREE COLLISIONS WITH THINGS THAT LANDED TODAY — flagged while the rules are still soft:**
1. ⭐⭐ **THE CONTENT-FINDER COUNTS MATCHES PER DISK; A/B PUTS TWO KERNELS ON ONE DISK.** orin's rule:
   *"multi-match counted per DISK; several disks with the kernel → root on the one the loader reported,
   else REFUSE loudly."* **A/B = two matching boot partitions on ONE disk** — `matches=2` where BOOTROOT
   witnessed `matches=1`, and *"which disk"* is the wrong question: it must discriminate **WITHIN** a disk.
   **The rule as written does not cover its own roadmap successor.** Decide REFUSE-vs-resolution before
   A/B forces it.
2. **No A/B slot on the card today.** `make-pi-img.sh` lays exactly two partitions — P1 FAT boot, **P2
   UnaFS in a FIXED 8 MB tail** (`UNAFS_MB=8`, hard-fails if exceeded), 64 MB image. A known-good copy
   needs a third partition or a second FAT ⇒ **moves the layout VOLID/LAYOUT/FITSLAND just settled.**
   A dependency, not an objection — but it belongs in the item, not in the builder's surprise.
3. ⛔ **THE SELF-CLONE INSTALLER DESTROYS THE KNOWN-GOOD COPY IT WOULD NEED.** Gate 3 lays a **fresh GPT
   over the WHOLE card** (GPT → zero ESP metadata → FAT32 → mirror back). **A/B needs one slot written and
   the other left intact; today's installer is whole-card by construction.** ⭐ **And it is the same
   function that IS the self-hosting primitive — the healing story and the install story collide in one
   place, and it is pi's.**
⚠ **Tree knowledge, honestly scoped: `tryboot|autoboot` hits 5 files** incl. `docs/dev/OS/01_BOOT_HAL/
arch_arm64.md`. **NOT verified which are real Pi-firmware knowledge vs coincidental Tegra matches**
(`fdt_tegra.rs`, `xusb_tegra.rs` are in the list). **Read properly at a focus turn — unverified count
handed over as unverified.**

**⛔ CORRECTION TO (1) ABOVE — PI QUOTED AN OVERRULED RULE.** *"root on the one the loader reported, else
REFUSE"* was **struck by Peter**: there is **no refusal and no loader tie-break — FIRST FOUND is root.**
And A/B on one disk is answered by the **STAMP, not the count**: a test kernel and a known-good kernel are
different builds, so the running image matches exactly ONE file. **The multi-match case pi raised does not
arise.** ⚠ **Pi cited a rule from a message hours old without re-checking it was still live — the day's own
class, in the direction of TIME (axis (b) of the round's check), not scope.**
✅ **THE REAL GAP UNDERNEATH IT SURVIVES, and orin credited it: `mount_source` mounts ONE FAT PARTITION PER
SOURCE**, so A/B-as-two-FATs needs **per-partition volumes** — a **shared-fs-lane** dependency, not pi's.
**THREE DEPENDENCIES OF THE ROADMAP ITEM: per-partition volumes (shared-fs) · card layout (pi) ·
whole-card installer (pi).**

⚠ **OPEN QUESTION SENT — two descriptions of the finder are now in play:** *"a WINDOW of its own running
image vs the candidate file's bytes"* (BOOTROOT witnessed `window_off=0x80000 window_len=4096`) vs *"the
file carrying its own BUILD SHA (frozen set item 5')"*. **Which is live decides whether pi's adopted BSS
requirement is still load-bearing** — *window must be a named `.text` range, never BSS_ — **is meaningful
ONLY for the window-compare.** If a stamp replaced it, **that requirement is stale and must come OUT of
the brief rather than sit there satisfied by accident.** ⭐ **A requirement outliving its mechanism is half
of today's errors; asked rather than assumed.**
**Edge noted, not an objection:** *"byte-identical clones are the same version, so first-found is
harmless"* — harmless for the KERNEL, but the mount is the **VOLUME** (`/boot` = the FAT the kernel was
found in). **Two slots, identical kernels, different volume contents — mid-promotion, refreshing a
known-good slot from a validated test slot — makes first-found arbitrary. Narrow, and it exists exactly
when healing is running.**

### 2026-09-09 · ⭐⭐ **THE SESSION'S OPENING QUESTION, ANSWERED — AND PI'S OWN ANSWER INVERTED**
Peter verbatim (02:05Z): *"if there's no good bootable kernel a user will hopefully be able to reboot into
linux, mac, or win to use our installer to assist in the initial healing process."*
⇒ **The host-side installer is a PRODUCT — cross-platform, in-repo, versioned — the LAST RUNG OF HEALING,
and the bench write scripts CONVERGE INTO IT.** `tools/unafs` (cited by pi this morning) is its seed:
already cross-platform per its own README, already in-repo, already driven by `arroyo`.
**THE LADDER, all generic: self-verdict → firmware fallback to the known-good disk (tryboot) → the user
boots another OS and runs UnaOS's installer.** Each rung lives BELOW the one above it, which is why it can
catch that one's failure.

⭐ **THIS CLOSES THE QUESTION THIS SESSION OPENED WITH.** orin 21 asked at hour one who owns the Pi's card
target-identity gate; pi answered *"the writer is unversioned bench tooling"* and routed **"should it be
in the repo"** to Peter as a **hygiene** call. **It was never hygiene. It is a product feature.** Every
morning finding re-reads: **`load-card10.sh`'s 644 lines of refusal machinery is the PRODUCT'S SAFETY
DESIGN SITTING IN SCRATCH**; `identify-card.sh` is the product's *"which disk am I about to write"* step.

⛔⛔ **AND IT INVERTS PI'S OWN RECOMMENDATION, WHICH IS RECORDED IN ORIN'S BULLETIN IN PI'S NAME.**
Pi argued **denylist — refuse iff JETSON or RMBP; UNKNOWN warns, never blocks** — because an allowlist
refuses a blank card and gets switched off. **Sound for a BENCH with three known cards. WRONG for a
USER'S MACHINE:** on a laptop the disks are the system drive, the backup drive, the photos — **none
classify as JETSON or RMBP, so a denylist writes to ALL of them.** At product scale the default inverts:
**refuse anything not POSITIVELY identified as target media; "UNKNOWN warns" becomes "UNKNOWN refuses."**
⚠ **The argument was correct and BENCH-SCOPED, and pi did not say so — the day's own class, in pi's name,
in a peer's artifact.** Asked orin to attach one clause: *"bench-scoped; the installer-as-product inverts
it."* **The property for the product rung, same as tonight's but harder: it runs on a machine we know
NOTHING about, so target media must be POSITIVE EVIDENCE, never the absence of a known-other.**

**✅ FINDER QUESTION ANSWERED — ONE MECHANISM, NOT TWO.** The compare is still a **named `.text` window
against the file's bytes at the derived offset** (what BOOTROOT witnessed: `window_off=0x80000
window_len=4096 file_off=0x0`); the **build stamp is a `.text`-placed constant that the compared bytes
COVER.** Item 5' reads *"include the build stamp IN the compared bytes"*, **not "replace the window."**
⇒ **PI'S BSS REQUIREMENT IS LIVE AND LOAD-BEARING, not satisfied by accident** — it stays in the brief.
⭐ **And the combination is better than either half: CONTENT identity (the window) + BUILD identity (the
stamp) in ONE comparison — which is exactly why A/B resolves to a single match.**
**✅ PI'S PROMOTION-WINDOW EDGE ACCEPTED and on the roadmap item:** identical kernels on two volumes with
different contents → **first-found picks a volume arbitrarily during exactly the window healing runs in.**
**The promotion step must make the slots distinguishable — a per-promotion stamp or a slot mark — BEFORE
A/B ships.** ⭐ **Asking rather than assuming was right: a stale requirement of pi's would otherwise have
sat in a peer's brief being satisfied by coincidence.**

### 2026-09-09 · **"WE HAVE A SUPERPOWER SYSTEM THAT CAN EXPORT PERFECTLY NATIVE APPS" (Peter 02:25Z) — the mechanism is the VESSEL**
⚠ **There is no artifact called an "export system."** `git ls-files | grep -ic export` → **0**
(control: `handler` hits **56** doc files, so the tree is greppable and the zero is about the word).
**The mechanism is `vessels/` — and a vessel already IS the native app.** Six exist: `aether-shell`,
`facet`, `lumen`, `phonolite`, `pulse`, `una`. `vessels/facet/README.md` verbatim: *"A **vessel** is an
executable a user runs: it wires together a Tokio runtime, the message bus, a selection of **handlers**,
and a native GUI window."* Reference vessel is `lumen`; architecture in `docs/dev/USERLAND/ARCHITECTURE.md`.

⭐ **SO THE INSTALLER'S PRODUCT FORM IS PRECISE, NOT METAPHORICAL: an INSTALL HANDLER composed into a
VESSEL, built per host OS.** Where each half lives:
- **`libs/fs/unafs`** — volume logic, already a library, already the KAT-pinned on-disk format the kernel mounts.
- **`tools/unafs`** — the CLI over it. **Peter's "seed" is a seed of the HANDLER, not the vessel**: its
  subcommands (`init`/`put`/`get`/`attr-set`) are handler operations with an argv front end bolted on.
- **The vessel** — wiring, lifecycle, native window. **The part that does not exist yet.**
⭐⭐ **AND IT PLACES THE DAY'S OTHER THREAD: the "which disk am I about to write" decision belongs in the
HANDLER, not the vessel** — one implementation, every host OS, ONE place to get the positive-identification
rule right. **The bench scripts' divergence — three per-platform writers, `flash-pi4.sh` and
`load-card10.sh` disagreeing on whether the target is checked AT ALL — is exactly what a single handler
removes.** That is the structural answer to the question this session opened with.
⚠ **Scope: `059e04db`, 94 behind — Ring 3 may have moved. And pi read the two READMEs, NOT
`ARCHITECTURE.md`: "vessel = native app" is THEIR words; composition details unverified.**

**✅ CLOSED:** orin verified in their tree — same six vessels, and **`ARCHITECTURE.md` names vessels the
same way**, which discharges the one thing pi left unverified. Recorded as the precise form: **an INSTALL
HANDLER composed into a VESSEL per host OS**; `libs/fs/unafs` the volume logic · `tools/unafs` the
handler's seed · the vessel the part that does not exist yet · **the "which disk" positive-identification
rule in the handler, ONCE, for every host.** ⭐ *"Export system" struck from their bulletin's WORDING while
Peter's phrase stays as his* — the right distinction: do not rewrite his words, do not propagate them as a
technical term either.

### 2026-09-09 · **SEAT ROTATION — rmbp 16 ARCHIVED · rmbp 17 TAKES THE FOCUS AT THE CAFE · orin 22 CLOSING**
Peter via orin 22: **rmbp 16 is ARCHIVED (context full) — messages to it will NOT be read.** **rmbp 17
starts at the cafe WITH THE FOCUS** (the rMBP travels — [[focus-rotates-the-rmbp-travels]]), **orin 23 is
its support seat.** orin 22 closes after INTEGRATE reports (baton + resume + close report, nothing new
spawned). **PI STAYS OPEN AS SUPPORT; the standing order is unchanged — no jobs, queue pi work.**
⚠ **pi's counterpart changes, pi's ROLE does not.** Nothing owed to rmbp 16; nothing held for rmbp 17.

**SENT TO ORIN 22 FOR ITS BATON (not just the bulletin — a successor reads the baton first):**
1. ⭐ **GRANT #10 IS TO THE WORK, NOT THE SESSION.** Unspent, does not expire with orin 22, and whoever
   picks up INTEGRATE/FOLLOWUP inherits the **same five conditions**: measured list from a real run ·
   **re-key only, no deletions, delete-only rows come back to pi** · floor arithmetic in the commit
   message (**pi = 120 at pi's tip; theirs reads higher from the fold — never quote across trees**) ·
   bounding rules **as the four-row table, not the prose** · PI-ROWS.md before the spec commit.
2. ⭐ **THE "ZERO ROWS" READING MUST TRAVEL WITH IT:** when the post-metal run reports no pi rows moved,
   **that is BLINDNESS, NOT SAFETY** (pi has zero rows naming `/usb` or `/volumes`, measured 3× today).
   **A successor reading a green list as "inert on pi" draws the wrong conclusion from a TRUE result —
   the day's whole lesson, and the easiest thing to lose in a handoff.**

**PI'S FOUR OPEN ITEMS NEED NO CARRIER — they live in THIS file:** grant #10 (unspent) · `/usb` write-posture
witness · tryboot/autoboot verification (hit count unverified) · `install/pi.rs` runtime-consent read under
the self-hosting frame. **This queue is the durable home; no bulletin is load-bearing for pi.**

**✅ HANDOFF SECURED — both items are in the BATON (`orin-23.md`), not only the bulletin:** the five
conditions verbatim in substance, **"to the work, not to a session"**, **pi floor 120 at pi's tip with the
never-across-trees clause**, the table not the prose, PI-ROWS.md before the commit — and **the zero-rows
reading as its own bold paragraph in pi's name.** orin 22 closes with: *no pi row moved, no pi file
touched, grant unspent, nothing owed either way.* **No further message sent — they are at context limit
and closing; the kindest handoff is silence once the record is secured.**

### 2026-09-09 · **ORIN 22 CLOSED — INTEGRATE landed `600887c2`; and its known gap is PI'S A/B EDGE AT ANOTHER SCALE**
`exec-orin22-bootroot` tip **`600887c2`**: HOMESOIL · LABELMOUNT · VERSIONWIN · QEMU-FAST with the
`<log>.run` sidecar · census fix. Focus gate green (check 59 legs 0 ❌; esp-jetson banner with sdmmc;
strings 12/12). **render11 staged, NOT flown.** Post-metal list in the baton (kernel8 both modes, finder
mutations, m1–m3, test-arm, WC). **No pi row moved, no pi file touched, GRANT #10 UNSPENT and in the
baton. Nothing owed either way.** orin 23 is support for rmbp 17.

⭐⭐ **THE CONNECTION NOBODY HAS MADE YET — orin's known gap and pi's A/B edge are ONE DEFECT AT TWO
SCALES.** Their gap, verified by grep and written into the baton: *"clone-vs-alias dedupe is NOT in — two
cloned cards would be deduped and one hidden from `/volumes`; root unaffected."* **Pi's promotion-window
edge, raised hours earlier: identical kernels on two volumes with different contents → first-found picks a
volume arbitrarily.**
**Both are the same root: CONTENT IDENTITY CANNOT DISTINGUISH BYTE-IDENTICAL VOLUMES.** Given two
identical things the system either **hides one** (dedupe, orin's gap) or **picks one arbitrarily**
(first-found, pi's edge) — and **which failure you get depends only on which code path reaches them
first.** ⇒ **A fix for one should fix the other; a fix for only one leaves the other live and looking
solved.** Both need the same thing: **something that distinguishes copies that content cannot** — a slot
mark, a per-promotion stamp, a volume identity that is not derived from the bytes being compared.
**HAND THIS TO rmbp 17 / orin 23 — it is the first job of the next focus turn on their side, and A/B on
the roadmap on mine. Neither seat currently knows the two are the same item.**

### 2026-09-09 · **POST-METAL STATE — render11 flew, batteries green, four pi items sharpened**
render11 FLEW on the Orin; root-by-content works on metal (`matches=1 home=-`, scorer11 6/6).
Full battery at `600887c2`: `kernel8-test` 0 (**125/125 at THEIR tip — pi's floor is 120, never
quoted across trees**), `test-arm` 0, `UNAOS_WC=1 test` 0. Fast mode reported inline:
`[fast: completion +12.3s grace 20s wall 32.6s]`, 5980 lines vs 77,932 in pi's full capture.
✅ **Pi's one flagged fast-mode concern landed exactly: `[shellup]` at `t=12908ms` falls ~0.6 s
after completion, inside the 20 s grace.** The default covers it.

**PI ITEMS, sharpened tonight:**
1. ⭐ **`/volumes` + `/usb` are entirely unwitnessed on pi — and it is DOCUMENTED-AND-UNGATED, which
   is stronger evidence than absence.** Measured: **2 COMMENT mentions, 0 directive rows**
   (`:2335`, `:2338` at pi's sha — a `root_prefixes(["/", "/fat", "/usb"])` block that **names the
   exact boundary case, `/usbfoo` must not be mistaken for `/usb`** — and no row tests any of it).
   ⚠ **A green "no pi row moved" after LABELMOUNT is a true statement about a gate that never
   looked.** Same shape as `wcg.rs` from the other side: there the contract sits where its consumer
   relies on it; here the property sits where nothing consumes it.
2. ⚠ **`[u7stk] headroom=` must NEVER be read from a fast run.** `hw=` is a high-water mark, so fast
   mode keeps the small early samples and drops the large late ones. **Owed item #4 (is 32 KiB still
   right?) needs `UNAOS_QEMU_FULL=1`.** Filed on orin's side too.
3. **Conditional: retire `pi4-regression.spec:36-40`** — the CAPSTONE 3-of-4-core caveat — **IF the
   APS executor makes secondaries join reliably on the Pi.** ⚠ Until then the caveat **pre-excuses
   the exact misses a regression would produce**, so a post-change pi run must be read with those
   five lines in view. orin's executor now quotes the `CAPSTONE \w+: PASS` count from its own run
   rather than the MBENCH total, and takes "no literal 0x3f/6" as a constraint (pi reads `0xf`).
4. **`--explain` delimiter (rmbp's, reported):** hand-quoting collides with **7 directive rows across
   the corpus that contain a single quote**, incl. `pi4-regression.spec:99`
   `COMPLETE :: SCHED: task 'el0-midden' -> core`. Their acceptance test runs on `jetson-jd5.spec`,
   which has none — **the population rule landing on a change made by the population rule.**

⭐ **THE FOLD WINDOW IS OPEN.** The Orin's metal result is in, which is exactly the window the
concurrency ruling carves out for other-arch work. Pi is **94 behind trunk**; Peter said he would
have pi fold at the cafe and the cafe did not happen. **Awaiting his word, not a lane call.**

---

## APPEND — pi 10, 2026-09-08: THE PI DISCARDS THE ONLY HARDWARE IDENTITY ITS CARD HAS

Derived in THIS tree (`hw-pi4` `247b1b95`, 0 behind trunk, clean), this turn, from source only.
Nothing here is relayed and nothing is from another seat's tree or baton.

**M1 — the Pi issues CMD2 ALL_SEND_CID and never reads the response.**

    sed -n '556,566p' unaos/crates/kernel/src/drivers/emmc2.rs

    // 8. CMD2 ALL_SEND_CID (R2, CRC on, index off) — moves the card to identification state.
    send_command(base, cmd(2) | CMD_RESP_136 | CMD_CRCCHK, 0).ok()?;
    // 9. CMD3 SEND_RELATIVE_ADDR (R6) -> rca in RESP0[31:16].
    send_command(base, cmd(3) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0).ok()?;

`CMD_RESP_136` is set, so the controller DOES latch the 136-bit CID into RESP0..RESP3 — and the very
next command overwrites it. Absence claim, population named and enumerated: all 16 occurrences of
`read_resp|RESP0|RESP1|RESP2|RESP3` in the file (`grep -n`), none between :557 and :560. The contrast
is four lines down — CMD9 SEND_CSD does `let resp = read_resp(base);` and parses capacity, so both the
136-bit read helper (`read_resp`) and the bit-field extractor (`csd_bits`) already exist in this file
and are applied to the CSD only.

**M2 — the same tree's OTHER aarch64 SD driver decodes it in full.**

    sed -n '546,549p' unaos/crates/kernel/src/arch/aarch64/sdmmc_tegra.rs
    // 9. CMD2 ALL_SEND_CID (R2) -> identification state. Decode + print the CID.
    let cid = read_resp(base); print_cid(&cid);

`print_cid` (:611–628) decodes MID / OID / PNM / PRV / **PSN (32-bit serial)** / MDT, and the card
struct stores `cid: [u32; 4]` (:394, :604). So this is a divergence between two drivers in one tree,
not a missing capability.

**M3 — what the Pi's block registry can therefore supply as identity: `num_blocks`, and nothing else.**
`BlockDeviceInfo` has 5 fields (`block.rs:121–127`). The Pi's sole construction site is
`emmc2.rs:631–637` (`grep -rn "register_sd("` → exactly one caller), and four of the five are literals:
`slot_id: 0`, `block_size: 512`, `vendor: *b"BCM-SD  "`, `product: *b"microSD Card    "`. Only
`num_blocks` varies with the card.

**M4 — INSTALL-SEL's "durable identity" is that same triple, and it is UNCONDITIONAL (no cfg on any
site — `install/selfguard.rs`, `install/mod.rs`, `video/instgui.rs`), so it is live on this board.**
`BlockDeviceId { handle, slot_id, num_blocks }` (`block.rs:569–580`); `lookup` (:587) accepts on
`slot_id == && num_blocks ==`. On the Pi, handle is Global and slot_id is the constant 0 sentinel, so
the discriminating power of the Pi's device identity is **the card's size**.

⚠ **THE ALARMING READING IS THE WRONG ONE — record it so nobody re-raises it.** The selfguard's
*verdict* is NOT keyed on geometry: `Cand.serials` holds "every FAT volume serial found on the device"
and that is what decides `BootDevice` vs `Eligible`. The geometry triple is only the key that maps a
chosen target to its cached verdict. A byte-clone of the boot card therefore classifies as
**BootDevice and is REFUSED** — fail-safe. And two same-size cards cannot collide in the candidate set,
because at most one SD + one USB are registered at once and `matches()` tests the `usb` flag first.

**THE OPEN QUESTION THIS LEAVES — NOT ANSWERED HERE, do not act on it as if it were:** the window
between `instgui.rs`'s frozen `PENDING` id and the engine's re-resolve through `block::lookup` is
proven only by (Global, 0, num_blocks). `emmc2::probe()` runs once on the BSP at boot. **I did NOT
establish whether any card re-probe or hot-plug path exists on this board**, and that is the fact the
whole question turns on: with no re-probe the registry cannot notice a swap at all, which is a
different (and larger) statement than a size collision. Enumerate that population before writing
anything down as a finding.

**PROPERTY, NOT MECHANISM — the CID question belongs to whoever owns the consumer.** What is measured
here is only: *the Pi's block registry exposes no per-card identity, and the hardware's unique serial
is already clocked into the controller's response registers and dropped at a named line.* Whether
anything should read it, where it should live, and whether `BlockDeviceInfo` (shared kernel core, not
pi's lane) grows a field are not pi's calls to make.

## APPEND — pi 10, 2026-09-08: two answers to orin 23, PI-RELEVANT HALVES ONLY

Derived at trunk in this tree (`247b1b95`, 0 behind, clean). The dock half is ORIN'S LANE — the term
and its cap are theirs; only pi's exposure is recorded here.

**A. `wm::dock_tiles()` under-counts the Pi's dock strip by ONE.** It mirrors `dock::pin_shell` only
(its own comment: *"SHELLPIN (integrator, GR27) — mirror `dock::pin_shell`"*), while the four readers
in `dock.rs` — `strip_rect` :586, `compose` :716, `press_at` :956, `selftest` :1207 — each apply
THREE pins: `pin_shell` → `pin_quarry` → `pin_pulse`. On `kernel8` quarry is off (`arroyo`: *"kernel8
never enables deskcascade, so quarry stays off there"*), so pi's exposure is `pin_pulse` alone —
declared `dock.rs:1314`, no cfg, runtime-guarded by `pulsewin::ever_armed()`, and armed by
`desktop_firmware::activate` (`desktop_firmware.rs:373`), which is the Pi/Orin seam. **So the aarch64
arm of `dock_tiles`'s own cfg is exactly the condition under which the pin it cannot see will fire.**
Consumers: `occ_clip` (wm.rs:17070) and `composite_inner` (:5124). `strip_rect` feeds `erase_clip` and
DOES apply all three — **erase clip sized from three pins, occlusion clip from one.**
Found only by symbol grep: `pin_pulse` is folded onto `pin_quarry`'s physical line at all four call
sites (PARITY.md §5.3, panic-`Location` line-neutrality). Source read only — no build, no boot.

    grep -rn "fn pin_" unaos/crates/kernel/src/video/dock.rs
    grep -n  "pin_shell(&mut" unaos/crates/kernel/src/video/dock.rs

**B. ON THE PI, Default (SD) AND Usb CAN BE THE SAME PHYSICAL DEVICE.** `block.rs:687`, the
`aarch64 + baremetal` arm of `publish_usb_geometry`, writes the same `BlockDeviceInfo` into BOTH
`USB_BLOCK_DEVICE` and `BLOCK_DEVICE` when `BACKEND != BACKEND_SD` — i.e. whenever no card registered
(`emmc2::probe()` found none, or identify failed). `block.rs:841` states the same reachability from the
other side. This is the x86 one-stick-two-handles collapse occurring on this board. In that state the
shared `slot_id` is the stick's LIVE xHCI slot, not the 0 sentinel — so a dedupe keyed on equal live
slot id gets it right. ⚠ **Do not carry the premise "different controllers, therefore disjoint" —
it is false, and the thing that makes the outcome safe is the live-slot rule, not the disjointness.**

## APPEND — pi 10, 2026-09-08: A SECOND INSTANCE OF THE UNANCHORED-MATCH HAZARD, AND IT WAS MINE

The queue already carries `CLAUDE.md`'s `awk '/pattern/'` idiom as unsafe for bracketed tags
(`awk '/[u7stk]/'` = 77,883 against a truth of 219). **This is the same family, from the other end:
not a regex metacharacter, a plain SUBSTRING with no anchor — and this time I published the bad
command as the proof of my own finding.**

To show `./arroyo state` exists on one unlanded branch I handed peers
`awk 'index($0,"state)")' unaos/arroyo`. Across the four heads it returns:

    origin/main 1 · origin/hw-pi4 1 · origin/hw-jetson 2 · origin/hw-rmbp 0

which reads as *"trunk has it, pi has it, rmbp does not"* — **the inverse of the truth.** Both stray
hits are one comment line containing `#[cfg(all(tegra, aarch64, supstate))]`; `supstate))` contains
`state)`. rmbp 17 ran it, got that table, and nearly refuted a correct peer with it — caught only by
reading the matched LINE instead of the count. Anchored form, verified here across all four refs
(`0 · 0 · 1 · 0`, the real dispatch being `origin/hw-jetson:7189`):

    git show <ref>:unaos/arroyo | grep -c '^[[:space:]]*state)'

⇒ **A COUNT IS NOT A FINDING UNTIL YOU HAVE READ ONE MATCH.** Same lesson as `a-check-that-cannot-fire`
from the opposite direction: there the pattern could not hit, here it hit things that were not the
thing. Both are answered by looking at the match, not the number.

## APPEND — pi 10: THE SECOND READER'S POPULATION DIFFERS FROM THE AUTHOR'S, AND THAT IS THE POINT

orin 23 confirmed both B108 answers at their tip and added the datum that makes the exercise worth
repeating: **`grep -n "fn pin_" video/dock.rs` returns THREE mint sites at trunk and FOUR on
hw-jetson** — theirs also has `pin_console` (:1402, PINCONSOLE, orin 17, unlanded). So the review gate
they took away is *"FOUR pins, ONE count, FIVE readers"*, and `wm::dock_tiles` must consume dock's own
tile count rather than carry a hand-rolled `+1`.

**Keep the shape, not just the result:** a second reader standing on a different tree does not merely
double-check the author — the two trees hold DIFFERENT populations, so each seat can only enumerate
what its own tree compiles. Neither enumeration was wrong; neither was complete alone. This is the same
per-tree law the queue already states for gate floors and baseline chains, arriving in a third place.

## APPEND — pi 10, 2026-09-08: THE DOCK/WM TILE IDENTITY MODEL AT TRUNK (orin's second population)

Orin's lane; recorded here because the WINID obligation binds any pi code that caches a `WinId`.

**Three axes.** `id: WinId` = u32 (wm.rs:180), live `1..=12`, `WIN_NONE` 0, pin sentinels descending
from `u32::MAX` (shell MAX, quarry MAX-1, pulse MAX-2) — ~4.29e9 headroom, no collision possible; the
second constraint is that a sentinel must not equal `PRESSED`'s idle value. `owner_asid: u64` is the
ROUTING identity and pins deliberately carry the REAL owner (`DockEntry.owner_asid` is *"what a tile
press raises"*), which is why `pulsewin::OWNER` had to be published — a pin under
`KERNEL_OWNER_DESKTOP` makes `pin_shell` read a pulse tile as a shell. **And there is NO generation,
by a recorded decision, twice** (wm.rs:920, :1914, :2212): ids are *"RECYCLED SLOT ALIASES with no
generation counter"*, and the answer taken instead is `winid_close_teardown` clearing every
REGISTERED holder cell ahead of the row free. Observed defect it answers: *"render7: the console's
win 1 came back as quarry's win 1."*

⇒ **THE OBLIGATION, and it applies to pi code too: anything caching a `WinId` must call
`wm::winid_register_holder` or it sits outside the only backstop there is.**

**Cross-check of registered holders vs candidate caches** (`grep -rn "winid_register_holder(&"` vs
`grep -rn "AtomicU32 = AtomicU32::new(wm::WIN_NONE)"`): registered = display_tegra `ORINWM1_WIN`,
fbcon `CONSOLE_WIN`, quarry `WIN`, instgui `WIN`, pulsewin `WIN`, wcg `SEAM_WIN`, wm `PROBE_CELL`(x2).
Unregistered = dock `PRESSED`, winmenu `BAR_OWNER` / `APP_OWNER` / `OPEN_OWNER`.
⚠ **The `*_OWNER` names hold WINDOW IDS, not owner asids** — read the declaration, not the name.
Three are safe on their own terms (PRESSED cleared in-arm; OPEN_OWNER re-checked each compose;
BAR_OWNER republished each compose). **`APP_OWNER` is open as a QUESTION, not a finding:** its
dismiss guard is keyed on `was != id`, so a close + same-slot re-issue between two compose passes
would republish the same id, skip the dismiss, and leave `Quit` reaping the wrong row via
`wm::close`. **Reachability NOT established** — that is the missing half and it is what any leg must
show first. Handed to orin as such; no remedy proposed (registration / generation / re-validate at
pick are three different answers and the choice is the owning seat's).

**PI'S OWN CARRY:** pi's dock is the same code. If a pi arc ever caches a `WinId` in a static — the
kernel8 desktop path has none today beyond the shared ones above — it inherits this obligation.

## ⚠ CORRECTION — pi 10, 2026-09-08: "THERE IS NO GENERATION" WAS WRONG. NO GENERATION *IN THE ID*.

The identity-model append above states *"there is NO generation, by a recorded decision, twice."*
**Strike that sentence.** I proved a statement about the ID and reported it as one about the SYSTEM.

    wm.rs:1914   "Window ids are RECYCLED SLOT ALIASES with no generation counter"   <- about the ID
    wm.rs:25415  static SLOT_GEN: [AtomicU32; MAX_WINDOWS]                            <- a generation EXISTS
    wm.rs:25423  pub fn winid_gen(id: WinId) -> u32                                   <- and is queryable
    wm.rs:22589  t.rows[slot] = row; let winid_generation = winid_slot_bump(slot);     <- bumped in create_inner

`SLOT_GEN`'s own header: *"per-SLOT reuse generation. **NOT part of the id** … this is **evidence only**,
the number of times the slot has been handed to a window, so a capture can tell the console that was
win 1 from the quarry that is win 1 now."* The bump site adds why packing it into `WinId` was refused —
**WC-B's syscall ABI, `dock`'s `WinId::MAX` sentinels, and F2's prior ruling** — and the block at :25246
is titled *"Why teardown-on-close and NOT a generation counter in the id."*

**Correct three-part statement:** (1) the id carries no generation, refused three times for three named
reasons; (2) a per-slot generation nevertheless exists and is readable as `wm::winid_gen(id)`;
(3) it is classed EVIDENCE, not correctness — teardown-clears-registered-holders remains the mechanism.
⇒ **The APP_OWNER remedy menu is therefore shorter than I wrote it: caching `(id, gen)` and comparing is
buildable today. Choosing it PROMOTES an evidence field to a load-bearing one — a real decision with a
documented history against it, not a new invention.** Sent to orin as a gate correction before it hardened.

⇒ **THE SHAPE, third instance today: cite the DECLARATION SITE, not the sentence that sounds like the
claim.** Same family as the unanchored `awk` and rmbp's zero-hit greps — an instrument that returned
something true about a neighbouring object.
