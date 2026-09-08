# rmbp 16 — the x86 FOCUS QUEUE (supersedes `../rmbp15/FOCUS-QUEUE.md`)

**What this is.** rmbp 16 is a SUPPORT round (R22: support spawns ZERO executors; R23: a stop on
starting jobs is never a stop on being support). orin holds the focus — inherited, not re-asked.
Peter's order at open: *no new jobs, support orin, and queue your work for your focus time.* This
file is that queue.

**When it fires.** The focus pivots to `hw-rmbp` the moment Peter leaves the bench, because the 2012
rMBP is a laptop and travels with him (`docs/dev/LAWS.md` §Focus). A trip round is the **only** round
in the fleet where x86 metal — boots, serial capture, card writes, staged media — is live, so the
queue is ordered metal-first. Nine executors is the ceiling, not a floor; queue past nine.

**Freshness contract.** Every number and line below was re-derived in this worktree at `1b24cb8a` on
2026-09-08, not inherited from rmbp 15's queue. **Three of the inherited numbers were wrong by then**
— they are corrected in place and flagged ⚠CHANGED. Re-run the same commands at pickup; the point of
the contract is that it keeps catching this.

---

## Q0 — BEFORE ANYTHING: B59 is TWO close-outs, and rmbp 15 tracked only one of them

rmbp 15's queue predicted: *"the real close-out is a fetchable branch, and it arrives when fold 8
(`f3e64daf`) lands on `hw-jetson`."* **Fold 8 landed. It did not close what the row claimed.**

    git merge-base --is-ancestor dc683c40 origin/hw-jetson   -> false
    git merge-base --is-ancestor 1aae3459 origin/hw-jetson   -> false
    git grep -l 'shell.relics.write_raw' origin/hw-jetson    -> 1 file
    git grep -l 'VfsBackend'             origin/hw-jetson    -> 3 files
    git grep -l 'layout_volid'           origin/hw-jetson    -> 0   (absence control: the grep can miss)

**CONTENT close-out and CITATION close-out are different things.** SHELLRELICS' and VFSROUTE's work
is on `origin/hw-jetson` — fetchable, safe, folded under re-cut shas. Their **commits** are on no
remote ref, so every review this lane issued against them cites a sha no peer can resolve: B58
measures `dc683c40`'s diffstat, B60's blocking condition is written against `1aae3459`. Nothing is
lost; the citations are dangling and only this machine can dereference them. **Remedy is a re-cite
against the landed shas, not a rescue.**

**The other half has NO close-out of either kind, and it is this lane's own work:**

| sha | what | on any remote? | content on any remote? |
|---|---|---|---|
| `28899d5c` | DUPGUARD fixture | no | no |
| `0019ec7a` | XHCINTD, the N-TRB re-arm | no | no |
| `e390721f` | PRTSCLOST — **a fifth commit B59 never named** | no | no |

    for r in origin/hw-rmbp origin/hw-jetson origin/hw-pi4 origin/main; do
        git grep -l 'kbd_dupguard_selftest' $r -- unaos/; done   -> 0 hits on all four
    git grep -l 'kbdwit' origin/hw-rmbp -- unaos/                -> 6   (positive control: the grep can hit)

These three are the **inputs to Q1**. **✅ CLOSED 2026-09-08 — Peter pushed `exec-orin17-dupguard`
mid-round**, and all three are ancestors of `origin/exec-orin17-dupguard` (verified by
`git merge-base --is-ancestor` after a fresh fetch, not by reading the ref's value). Every seat can
fetch the metal flight's inputs; the `prune`+`gc` hazard on this half is gone. **What is still open
is the CITATION half above** — B58's and B60's shas resolve only on this machine, and the remedy is
a re-cite against the landed shas.

**And the census instrument was wrong, which is why the fifth commit was missed.** B59 was built by
enumerating NAMED GRANT TARGETS. The derivation from the other end is one command:

    git rev-list $(git for-each-ref --format='%(refname)' refs/heads/) \
             --not $(git for-each-ref --format='%(refname)' refs/remotes/) | wc -l

**414 stranded commits across ~270 branch tips** at `1b24cb8a`. Most are superseded exec branches
whose content landed under other shas — the number is not itself a finding. The finding is that a
list of names cannot answer a reachability question, and a reachability sweep can. Ledgered B83.

---

## Q1 — THE rMBP METAL FLIGHT (J3). Metal-only. First, because it is why the round exists.

XHCINTD is **accepted** and blocked on nothing but this flight (B56). The completion path of the
keyboard interrupt-IN transfer has coverage **nowhere else in the fleet** (B45): x86's `kbdwit` is
EHCI and its xHCI HID device is a pointer; `test-arm` enumerates a real xHCI keyboard but QEMU sends
no reports without an injector `arroyo` does not have. **The rMBP at the glass is the only scorer.**

- Apply order XHCINTD then DUPGUARD, both `-3`. Regenerate from the commits, never from scratch paths:
  `git format-patch -1 0019ec7a --stdout` / `-1 28899d5c --stdout`.
- ⚠ `0019ec7a` also touches `arch/aarch64/display_tegra.rs` (14 lines). That is orin's lane —
  read it before applying and negotiate if it survives the rebase.
- Gate before the card write: `./arroyo check` both arches, `UNAOS_WC=1 ./arroyo test 150` with `wc`
  in the **build log**, `./arroyo test-arm`.
- Score the boot by the **loaded image's `max_vaddr` span** first. A 10/10 card sha proves the WRITE.
- Ride-alongs, each an open ledger row and the machine is open anyway: **A6** `[clickroute] -> FAIL`
  (deterministic on metal, green in QEMU — bracket it new-with-arc or pre-existing) · **A5** shell
  window tearing under storm · **A1** the BAR1 wedge recovery path · **A9** the FTDI bulk IN 0x81
  that is never driven, which is what blocks DEV-LOOP.

## Q2 — CLOSED during rmbp 15. Not queued.

SHELLRELICS **accepted** (its owed `write <lba> <byte>` leg arrived as `shell.relics.write_raw`, read
here at fold-8 tip rather than accepted on report); VFSROUTE **accept with one blocking condition**
(B60 — `same_volume` decides identity by name while the tegra bind mounts one card twice). Both
records are in `../rmbp15/LANDING-REPORT.md`. What remains is the Q0 re-cite, above.

## Q3 — J1, THE LANDING. Needs the adversarial panel, which is a fleet, which is the focus.

⚠**CHANGED. `git rev-list --count origin/main..hw-rmbp` = 166, not the inherited 121.** Still 94
behind; merge-base still `f49ea1e7`. The delta is rmbp 15's own 45 ledger commits
(`git rev-list --count 5c3dbb7e..hw-rmbp` = 45). **The arc grew 37% in one round in which no kernel
code was written** — the landing's price is a function of rounds waited, not of work done, and that
rate is now measured rather than asserted.

⚠**CHANGED. The known non-union grew too: `drivers/xhci/mod.rs` diverges 21/86 over 22 hunks**
against `origin/hw-jetson` (`git diff --numstat hw-rmbp origin/hw-jetson -- <file>`), not the
inherited 12/77. Budget an executor for that reconcile alone; it is the largest unknown in J1.

- COI guard holds: the author seat never reviews alone. Panel → ccd announce → peer ack from at least
  one other track seat → this seat's own `--no-ff` merge → trunk battery, with a **fresh `ls-remote`
  in the same turn as the merge**, both seats.
- **J1's price, in the form that is actually true (B88).** Not 166 commits of review — **the fleet is
  running SEVEN fewer gates and an older GATE-LEDGER because of this branch.** `k8-reach.py`,
  `k8-reach.registry`, `k8-modtree.py`, `knob-leg-covered.py`, `check-roots.sh`, `arch-families.sh`
  and `append-position.sh` exist on `hw-rmbp` and on **no other head**; `ledger-check.sh` is
  **+6/−190** against both `origin/main` and `origin/hw-jetson`, so the other seats validate their
  ledgers with a gate missing rmbp 12's four mutation-proven fixes. Eleven files diverge in total.
- **And this lane's FIXES are not reaching the fleet either (B71):** orin's `cargo test -p foreman`
  reds because `cc283929` (lookahead fix, unit-tested 8/8) is on `origin/hw-rmbp` and no other head —
  still red at their tip on 2026-09-08, their run. **Two instances in one day makes it a pattern:
  every other seat pays for this branch staying unlanded.**
- **Landing checklist item, agreed with orin 22:** their arc lands first, so the `UNAOS_NOSDMMC`
  registry row (`UNAOS_NOTEGRASMP` shape, status "deliberately unarmed on Pi builds — sdmmc is
  tegra-only") is **this seat's to add in the landing merge**, or `check` reds on a knob the gate has
  never seen.

## Q3b — THE x86 HARNESS LIES ABOUT THE BOOT TOPOLOGY (B87). New this round; do it BEFORE trusting any x86 boot gate.

Peter, 2026-09-08: *"the card is the hard drive … boot cold, boot dumb, presume nothing about the
machine."* orin 22's BOOTROOT makes every UEFI arch find its root by walking to the boot volume.
**x86 was scoped out of that arc for a reason this lane owns.**

- Under QEMU the boot ESP is deliberately a separate `-drive` and the kernel's `Default` is the
  `usb-storage` stick (`drivers/block.rs:918-919`, choice made in `builder/src/main.rs`), and **no
  AHCI/SATA/IDE block driver exists** — `drivers/` is `sdhc.rs`, `emmc2.rs`, `ehci`, `xhci`. So the
  dumb walk finds no root in the harness.
- **But metal already matches the walk**: `drivers/block.rs:182` — *"machine boots from a USB card
  reader, so the global slot IS the boot volume."* On the rMBP the boot volume and `Default` are the
  same device. **The divergence is the harness modelling a box we do not own.**
- **So this is not convergence blocked on a missing driver. It is: make QEMU present the ESP the way
  metal does, then converge on `fat::locate_boot_volume(serial) -> Option<BlockSource>`** (orin's
  shared helper, landing with BOOTROOT).
- **Why it is ordered before the gates: after BOOTROOT lands, an x86 QEMU boot exercises a topology
  metal does not have, so a green gate says nothing about the boot path it is named after.** Same
  family as everything else this round — the instrument does not model the thing it measures — except
  this one lives in this lane's harness rather than in anyone's code.
- Files: `unaos/arroyo` (the x86 QEMU drive lines), `builder/src/main.rs`, and whichever x86 specs
  assert the current two-device shape. Prove it by mutation at the sha it will run on.

## Q3c — THE 81x ON `check` (B92). Shared tooling, highest value-per-line in the queue.

`./arroyo check`'s 56-leg cfg matrix: **557.2 s cold, 155.0 s warm-serial, 1.9 s at P=6 with per-slot
target dirs** (orin 21's measurement, cause isolated by forcing all legs onto one slot — same source,
no edit, 8.0–11.9 s per leg). A feature set is part of cargo's fingerprint, so legs sharing a target
dir invalidate each other in turn.

**`arroyo` already documents this remedy in three other places** (`:2062`, `:5045`, `:2452` — the
`target/{x86,x86-pinlo,…}` split) and does not apply it to `KERNEL_CFG_MATRIX` at `:2683`.

**Ordered above the gate work because `check` both arches is in the DONE gate of every arc on every
track — 153 s per check per seat per commit is the fleet's iteration speed, not a build nicety.**
Coordinate with orin before cutting: their BUILDPERF arc found it and may take it.

## Q4 — THE `arroyo` SWEEP THIS LANE OWES (B55). Code-only, one line-neutral commit.

Re-verified at `1b24cb8a`: all eleven still restate the gate as `any(baremetal, tegra_el0)` in prose
— `unaos/arroyo` lines 906, 1011, 1046, 1078, 1175, 2802, 2956, 3047, 3235, 3553, 4076.

The executable half is the point: `unaos/arroyo:2143`'s `case ",${_af#--features }," in` with
`*,baremetal,*|*,tegra_el0,*)` is still the hand-maintained enumeration, while
`unaos/scripts/k8-reach.py` already computes the closure (`cargo_implications`, defined :214, called
:247). **Derive the case from it.** The commit carries its own grep, and `panic::Location` embeds
source lines, so line-neutral or tail-append only.

## Q4b — THE x86 SPECS ARE WIRED TO NOTHING (B95). Above the gate work, because it decides what the gate work is worth.

**No `arroyo` verb runs any x86 spec.** Every `--spec` call in the tree is a Pi one (`:6477`
`pi4-regression.spec`, `:6500` `$k8_spec`); the x86 verbs assert via `DEFAULT_FORBIDS` (`:2259`) and
never open a spec file. So `x86-witness.spec`, `x86-wc.spec`, `x86-fat.spec`, `x86-holocron.spec`,
`x86-wifival.spec`, `rmbp-boot.spec` and `round6-rmbp.spec` are operator-invocable through the
`mbench` passthrough (`:7287`) and exercised by no DONE gate.

**Prose already treats them as live** — `arroyo:611` on what `x86-wifival.spec` "asserts", five
`engine.md` citations of `x86-witness.spec` pinning invariants.

**Do this before Q5's junction/gate work:** B85 verified six FORBIDs could fire against their
emitters and never asked whether the file runs. Wiring one spec into the x86 verb is worth more than
perfecting rules inside a file nothing opens — and the wiring is where a RED-first proof belongs.

## Q5 — GATES THIS LANE OWES. Code-only, parallelizable, one executor each.

- **B63 — GATE-LEDGER's column-count blind spot.** Confirmed absent at `unaos/scripts/ledger-check.sh`
  this turn: no field-count assert exists. Three malformed rows are in the tree NOW (B16 at 6 fields,
  B40 at 8, B24 at 11). One line asserts each data row's field count equals its header's and names
  both counts. **It needs BOTH the assert and a sanctioned way to write a pipe in a cell** — B40's
  cause is a shell pipeline in an evidence cell, which is this lane's own standing convention, and
  `\|` does not save it. The pipe convention is Q7 (Peter's, two seats routed it to him).
- **B64 — GATE-K8REACH's blind sibling class.** The gate builds its universe from `arroyo`'s `_feats`
  lines, so a knob delivered by `option_env!` is outside its DOMAIN. Confirmed this turn:
  `grep -c UNAOS_DMAWIN unaos/arroyo` = **0**, while `arch/aarch64/rtl8168_tegra.rs:4255` reads
  `option_env!("UNAOS_DMAWIN")`. `UNAOS_FBW` / `UNAOS_FBH` are the same class. One pass: enumerate
  `option_env!` reads from source as a second universe and rule each one.
  **⚠ Class NARROWED 2026-09-08, by being wrong at a peer: NEGATIVE knobs are NOT the blind spot.
  `arroyo:982` spells `UNAOS_NOTEGRASMP` as a guard ON a `_feats` line, so the parse sees it and
  `k8-reach.registry:53` already carries its row. **A knob is inside the universe if it appears on
  a `_feats` line in ANY polarity**; the blind spot is `option_env!` reads armed by no `_feats`
  line at all. Same check found a live gotcha for orin: `k8-reach.py:25` REDS an unregistered
  knob, so `UNAOS_NOSDMMC` needs a registry row in the same commit that adds it.
- **B47 — `HOST_VERBS` ↔ dispatch-arm, in BOTH set directions.** Table at
  `unaos/libs/sys/midden_core/src/lib.rs:245` (use sites :336, :491), arms in `shell.rs`. Two verbs
  answered "Unknown command" for their entire existence while a comment claimed the invariant held.
  A gate that **reads** midden_core needs no grant; only an edit would.
- **B53 — the `[wc-d]`/`[wc-g]`/`[wc-h]`/`[wc-k]` fixtures do not model the console window**, giving
  them a run-to-run VARIABLE forbidden set. Specs in `unaos/scripts/specs/x86-wc.spec`.
- **B85 — six junction-keyed FORBIDs in this lane's specs.** aarch64 lost one to a sibling emitter
  interposing a token between the two the rule keyed on; a FORBID that stops matching is a false
  green and announces nothing. Four verified still-fireable here against `syscall.rs:7272` and
  `wm.rs:16342`/`:16350` (both arms); **`[wc-d] paygo` and `[wc-h] rollup` remain unverified.** The
  gate: assert every FORBID matches its emitter's current format. **Census closed at 6/6 verified,
  0 defective** — but the point is not the census: **`video/wcg.rs:3920-3925` already documents the
  contract** (fields MAY be inserted between matched keys; nothing renamed, reordered, or moved past
  the verdict), so these six are simply the x86 FORBIDs written stricter than the contract their
  emitters honour, and an emitter can invalidate them at any time while breaking no stated rule.
  **⛔ CHOOSE THE BOUND BY WHAT THE SIBLING TOKEN STARTS WITH, never by habit (pi 9, executed; verified
  independently here).** `\b` guards only against WORD-character extension: `span_blocks=2048\b` does
  NOT match `span_blocks=20480` (digit = word char, REKEY's case, sound) but **`scope=window\b` DOES
  match `scope=window-band`, because `-` is a non-word character and a boundary exists right there.**
  When the sibling differs by a non-word character, `\b` is INERT and `(?=\s|$)` is mandatory.
  **And never a trailing space: `mbench.py:256` strips every spec line while `Directive.__init__`
  compiles verbatim (`:157`), so a space bound vanishes and the rule false-hits the longer token.**
- **B93 — the mid-line-comment gate.** `main.rs:2051` is 933 chars with four statements packed before
  the first `//` (deliberate: a same-line append moves no `panic::Location`). Safe today, and the
  invariant is REMEMBERED not GATED. One mechanical check — no `;` after the first `//` outside a
  string, any line — protects it permanently and would have caught the original A9/PRTSCR-ORIN bug.
- The `supstate` × `holocron` / `orintenant` / `orinladder` matrix gap.
- Standing rule for all of them: **a check that cannot fire is not a check** — printing is not
  gating, a zero-hit result indicts the pattern, and each gate is proved by MUTATION at the sha it
  will run on.

## Q6 — SMALLER, ALL LEDGERED

- **B74 TEARSCOPE's fix arm — TAKEN. Contract met by CLAIMCHECK-2, and my third condition was a
  bad ask.** `strip::erase_rect` can DECLINE and three call sites discard it (`dock.rs`,
  `menubar.rs`, `crystal.rs`), after which `SLOT.store` forgets the debt. The census is now pinned to
  an identified boot — **boot 37 of 39, image `0x33f480` via `unknown.log:17778`; in
  `boot-render9.log` stop at file line 19966, since 19967+ is boot 38** — and all five numbers
  reproduce under `rollup ∧ scope=X` on that boot and no other. **Per-call-site decline counts are
  unobtainable by construction: `orin.log` closed 34 minutes before the emitter was authored, so the
  instrument postdates the capture.** They become a render10 measurement, i.e. the first boot
  carrying this fix — the arm and its measurement ride the same image, which is better than what
  this seat specified. **Observability first: never condition an arm on a measurement whose
  instrument does not exist yet.**
- **`strip.rs:820`'s "read torn=0 all boot" is FALSE and is this lane's to correct at fold time** —
  boot 37 has seven `torn=1` `win=1` rollups (emits 30–36), the only ones in the file, appearing
  ~20 s AFTER the close. A window tore; no bar was scored either way, so TEARSCOPE's conclusion
  stands and only the sentence is wrong.
- **B74's second defect** — `[wc-k] rollup scope=fills` renders a VERDICT on `samples=4`, so it reds
  under host load. It wants `INSUFFICIENT-SAMPLE` as a state distinct from a verdict.
- **`UNAOS_GIT_SHA`** — stamped into every build, read only by a Pi-only default-off HTTP body.
- ~~`partitions.md`'s "aarch64 only, because `fs::unafs` is" is false~~ — **REMOVED, B84. The
  sentence is TRUE and the flag was wrong**: it names the kernel MODULE `fs::unafs`, declared once at
  `fs/mod.rs:36` under `#[cfg(target_arch = "aarch64")]` on every head checked; what is unconditional
  is the arch-neutral library CRATE of the same name at `unaos/libs/fs/unafs/src/lib.rs` (zero
  `target_arch` gates). Do not edit the doc.
- **B58's `video/prtscr.rs`** — still FAT-direct; the module doc at `:17` says so in as many words.
- **B10 — the R19 shut-out register. A READING task, still never started**; its executor was killed
  at minute four three rounds ago. R19 is why it matters: failed paths stay open, and this register
  is what keeps them open.
- The older x86 rows: A2, A3, B1, B6, B7, B9.

## Q7 — PETER DECISIONS. Surface ONCE; do not chase.

- **A4** — the card as the default startup volume. Not kernel work: `bless` / Startup Disk on his
  laptop, and it is what makes unattended reboots into UnaOS possible (R3: nobody is there to hold ⌥).
- **B7** — the vug arbiter.
- **The ledger-cell pipe convention** — two seats routed it to him independently. **Do not edit
  CLAUDE.md.** Blocks half of B63.
- CONSOLETEXT's first mint stays black (his render6 ruling); reversing is one line (B54).

---

## What rmbp 16 REMOVED from this queue, and why it matters

**Two items, not one.** rmbp 15's queue and the rmbp-16 baton both carried: *C15 collapses the arming polarity, so
`#require=sdmmc` stops discriminating and this seat owes a new staging predicate.* **Retracted —
B82.** `esp_jetson()` forces `tegra` (+`tegrasmp`) and never adds `sdmmc`; `sdmmc` enters only via
`UNAOS_SDMMC=1` (`arroyo:1682`) or `UNAOS_SDMMCROOT=1` (`:1738`). Verified here at this seat's own
tip after orin 21 derived it from the other end. **A queued work item that existed only because of a
wrong premise is deleted, not scheduled** — which is the cheapest thing verification ever buys.

The second is the `partitions.md` doc fix, **retracted as B84**: the flag read the arch-neutral
library crate `unafs` and the doc's sentence names the aarch64-gated kernel module `fs::unafs`. The
"fix" would have introduced the falsehood it was sent to remove. **Two queued items deleted by
verification this round, zero scheduled** — and all three of this round's findings (B81's legibility,
B82's matrix leg, B84's shared name) are the same class: the wrong object is the one easier to reach,
and nothing in any of them malfunctions.

## Nine-executor shape, if the pivot comes with the fleet

1 metal (Q1, the seat itself at the glass) · 1 `xhci/mod.rs` 21/86 reconcile (Q3) · 2 landing panel
(Q3, adversarial, independent) · 1 `arroyo` derived-case sweep (Q4) · 3 gates (Q5, one each) · 1 B10
register (Q6). Q0's re-cite is the seat's own reading and costs no executor.

**Two briefing rules the orin seats paid for; adopt them on the pivot.** `git rev-parse HEAD` as
every executor's mandatory first command — orin 20 had four of fifteen handed a stale base, orin 21
six of six, and the line caught all ten. And **a cited SYMBOL is a claim, not metadata**:
`layout_volid` was carried by three seats on the day all three adopted "file plus symbol, never a
bare line number", and it does not exist. One grep verifies it — with an absence control, so the
grep proves it could have hit.
