# rmbp 13 — LANDING REPORT (2026-09-06; orin holds the focus; SUPPORT seat, no executors, no metal)

Written before the close rather than at it: sessions die mid-thought, and "I'll write it at close" is
a bet there will be a close. Updated in place while the seat runs on.

**Posture, stated once.** orin holds the focus (inherited from rmbp 12's close and orin 15/16's round,
not re-asked — memory `focus-is-inherited-never-reasked`). This seat is SUPPORT: **zero executors,
zero arcs, zero spawns** (R22), and the support role itself never paused (R23). Peter's instruction
opening this session was "do not start jobs you are supporting focus arch jetson orin"; that names a
scope, and grants, verification and owed deliverables run at full pace inside it.

**Nothing landed to trunk.** `main` is `f49ea1e7`. J1, the 72-commit rmbp landing, still needs an
adversarial panel, a panel is a fleet, and a fleet belongs to the focus seat.

## State at pickup — fresh host check, run in the opening turn

    main f49ea1e7 · hw-jetson bd7e13c5 · hw-pi4 2136ccab · hw-rmbp 0bb37470

`flatpak-spawn --host git ls-remote --heads origin` plus `git fetch origin`; `hw-rmbp` local tip equals
origin, tree clean. **Pushes Peter owes: NONE at open.** (`git log origin/hw-rmbp..hw-rmbp` empty.)

## J7 — the two items owed to orin

**Half one, `Folded-by:`, was already delivered; the baton's queue line is stale.** Verified rather
than re-written: `docs/dev/EXECUTOR-BRIEF.md:31` carries *"If the seat folds your diff by hand, the
seat adds `Folded-by: <seat>` above the Co-Authored-By line"*, added by `aaaa62a0`, and it is present
on **all four refs** — `origin/main`, `origin/hw-jetson`, `origin/hw-pi4`, `hw-rmbp` (`git show
<ref>:docs/dev/EXECUTOR-BRIEF.md | grep -n 'Folded-by'`, line 31 on each). Nothing to do.

**Half two, `./arroyo knoboff <feature>`, did not exist and does now.** `grep -n knoboff unaos/arroyo`
was empty; the brief has been telling every executor "when it exists, run it and quote the result"
since `aaaa62a0`, which means the one step that MEASURES the knob-off byte-identity rule has never
actually been run — executors proved the append POSITION and reasoned about the bytes. Full design
rationale is in the tool's own header and in ledger row **B17**; the four decisions that are the
content of it:

1. **The `llvm-objcopy -O binary` LOADABLE image**, never `kernel.elf` (build-path and debug metadata,
   plus LLVM's `.llvm.<hash>` .strtab suffixes — JB11 measured a pair differing by one loadable byte
   and 32 non-loaded ones) and never the ESP, which embeds SRC.TGZ and so differs whenever the source
   differs.
2. **Both builds in ONE directory** — a throwaway worktree under `~/unaos-bench/scratch/knoboff/` —
   because a build's own path reaches its artifact. XUSBFW's measurement says "in ONE directory" and
   `orindesk`'s says "same worktree path" for exactly this reason; a baseline built somewhere else
   puts a path delta and a code delta into the same number.
3. **`git diff HEAD` snapshot, never `git stash`** — one stack, shared by every worktree of this repo.
4. **An armed control probe, and a refusal without it.** Every run also builds the tree with the
   feature ON and requires that image to differ on at least one arch. Without it, a misspelled
   feature, a feature whose sites are all gated on a second feature nobody armed, and a knob this run
   never passed to cargo would each report "byte-identical" in the same words a real pass uses.

Exit status is the deliverable — 0 identical, 1 the image MOVED, 2 no verdict — and the dispatch arm
carries `|| exit $?` so a non-zero verdict cannot fall through to the script's own 0. That is B12's
piped-gate lesson one layer down.

**Scope, stated in the tool and printed on every PASS:** the x86_64 and aarch64-virt KERNEL images
only. NOT `kernel8.img`, which compiles from `kernel8()`'s curated `K8_FEATS` — a separate list that
deliberately does not draw from the knob map, which is SR1's open class — and no staged media. A green
knoboff is not a pi ack.

### Self-test, five ways, each quoted

| run | command | result |
|---|---|---|
| usage | `./arroyo knoboff` | exit **2**, usage |
| undeclared | `./arroyo knoboff notafeature` | exit **2**, names all 155 declared features |
| armed env | `UNAOS_TEGRA=1 ./arroyo knoboff bsptick` | exit **2**, prints armed set vs default set |
| PASS | `./arroyo knoboff wc` (baseline `0bb37470`) | exit **0** — x86 flat `1923bcd2…` 1,452,057 B, arm `2ef8e1eb…` 1,509,164 B, control fired on both arches |
| **GO-RED** | one blank line at `allocator.rs:3`, then `./arroyo knoboff wc` | exit **1** — BOTH images MOVED, same size, exactly **1 byte** differing each |

That go-red is the whole point of the tool in one number: a single inserted blank line moved one
`panic::Location` line number by one, and the check caught it at byte precision on both arches.

### Two things the test run taught, which are the part worth keeping

**A guard that fires on its own script.** The first cut of the environment check scanned the
environment for `UNAOS_*` and refused if any were set. It fired on **every** run — because BUILD-SHA-1
at `arroyo:54` `export`s `UNAOS_GIT_SHA` itself. A guard everyone has to pass an override to is a
guard that has stopped checking anything. It now asks the question it actually means — *is the armed
feature SET the default set?* — by re-entering `$0` with every `UNAOS_*` stripped and reading the
`⚡ kernel features:` banner back, so the 1400-line knob map stays the single source of truth and no
second copy of it lives in the function. (`UNAOS_GIT_SHA` is itself `option_env!`-embedded in the
image; all three builds inherit one arroyo's value, which is why knoboff drives cargo directly instead
of calling the baseline tree's own `./arroyo`, whose sha would be its own.)

**The first go-red mutation was INERT, and that is a fact worth a ledger row.** A blank line at
`main.rs:6` produced a byte-identical image and the tool said so. Not a tool failure: `strings -n 6` on
the flat images finds 88 `.rs` path strings in the x86 image and **zero** from `src/main.rs` — main.rs
contributes no `panic::Location` to either default image, so a line shift there is free at that
config. Recorded in the ledger's §D, because it independently corroborates the S7 step-1 x86-identity
argument, which reasoned from cfg-gating; this measures the stronger fact.

## Gate

Run on the committed tree, exit status read directly and not through a pipe:

- `cd unaos && ./arroyo check` — **exit 0**. Legs: x86_64 OK · aarch64 OK · bootloader OK ·
  GATE-FAMILY OK (8 platform-split families, none grown) · GATE-KNOB OK (155 features declared, 154
  named by a cfg, 0 phantom, 0 dead) · GATE-ROOTS OK (9 binary targets, each named by a leg) ·
  GATE-LEDGER OK (115 rows) · knob→leg coverage OK · knob→builder wiring OK · kernel cfg coverage OK
  (45 legs) · userspace x86_64 OK (4 crates) · userspace aarch64 OK (5 crates) · midden_core tests OK.
- `unaos/scripts/ledger-check.sh` standalone on the final content — **exit 0**, 115 rows.

An earlier `check` run was **orphaned** — its wrapper died and the process went with it, so it produced
no readable status. It is recorded here as what it is: not a gate result. The run above is the gate of
record, and it was started after every edit in this commit. It also competed with orin's nine-executor
fleet for the machine, which is why it took as long as it did.

Changed files: `unaos/arroyo` (the tool), `docs/dev/EXECUTOR-BRIEF.md` §5 (the hedge removed — it now
tells executors to run it and quote the exit status), `docs/dev/OS/rmbp-ledger.md` (B17 + the §D fact).
No Rust changed, so no kernel behaviour moved; the go-red run above is the proof that the two kernel
images are bit-identical to `0bb37470` with the mutation reverted.

## Grants issued this round — two asks from the focus seat, answered in one reply

orin 16 opened a nine-executor round and sent two patch-first asks mid-session. Grants are the support
seat's product; answering them is not "starting jobs", and leaving the focus seat blocked would have
been R23 again.

| ask | verdict | the condition that mattered |
|---|---|---|
| **BSPRUN-shared** (`arch/aarch64/sched.rs`, `8e864463`) | **GRANTED** | the **P7 trap**, checked rather than assumed: on both appended lines the statement sits BEFORE the line's first `//`. After it they compile nothing while the gate stays green |
| **DESKFIX (a) A30** (`video/pulsewin.rs`, `video/dock.rs`, `0fced841`) | **GRANTED** | the defect reproduces here: `close()` never clears `ARMED`, which `service()` reads every render pass |
| **DESKFIX (b) SO5** (`pal.rs`, filed unapplied) | **REFUSED as written, reshaped** | `sprite_scale`'s callers **enumerated**: `extent()` → `vug.rs:884`, arch-neutral, so the "aarch64-local" fix reaches the rMBP |
| **PRTSCR-ASYNC** (`video/prtscr.rs`, `a86e3268`, SR2) | **GRANTED with one BLOCKING correction** | `Job::begin()` mounts `mount_program_source()` — the single-rung call PRTSCR-VOL replaced. As written the async capture refuses forever on the one board whose 70 s number *is* SR2 |

**What was verified first-hand rather than accepted on report.** For BSPRUN: the executor's stale-row
finding is correct (`ea182855` and `a28879de` are ancestors of origin/main, origin/hw-jetson and
hw-rmbp — six `--is-ancestor` checks, all true); the hunk profile is as stated (`-1743,7 +1743,7`,
`-10455,7 +10455,7`, and `-10659,3 +10659,101`, a genuine tail append); the single semantic change is
ordering-only, because `mark_online` (`sched.rs:3142`) is a bounds-checked idempotent store that
`run_bsp` calls again at `:6476` with nothing in between; and `el0_placement_possible`
(`sched.rs:3394`) really is `any(c) { ONLINE_MASK[c] && el1_core(c) }`. **`git apply --check` limited to
the sched.rs half returns rc 0 against hw-rmbp's own copy**, so the shape is not base-specific. One
correction sent: that predicate lives at `arch/aarch64/sched.rs:3394`, not `syscall.rs:8335`.

**The refusal is the part worth keeping.** SO5's patch drops the `+1` from `pal::cursor::sprite_scale`.
`sprite_scale` has exactly two callers — `extent()` at `pal.rs:390` and `paint()` at `:739` — and
`extent()`'s caller is `vug.rs:884`, which carries no arch gate and is therefore live on the rMBP.
Dropping the `+1` shrinks x86's cursor bounding square from 9·(s+1) to 9·s, and `pal.rs:386-388`
records that this square's sizing was itself the fix for the midden cursor trails. The patch would
have fixed the Orin by regressing this board. The reshape converges **upward** instead — raise
`cursor.rs:3803` to `sprite_scale`, leaving x86 untouched by construction — and that hunk is this
seat's to cut and fly at rMBP panel geometry. orin's root cause is confirmed sound either way:
`SPRITE_OWNS_PAINT` is `cfg!(target_arch = "x86_64")` at `pal.rs:301`, so the "the two never coexist"
premise really is false on aarch64. Their committed `[sprite]` witness (`same=0`) is wanted as-is.

**ASK #3, PRTSCR-ASYNC — and the apply failure that turned out to be the finding.** The design is
sound: `capture_inner` becomes a `Job` state machine, `service()` advances one bounded slice per
device-service pass, `capture()` stays synchronous as a driver of the same machine so the `screenshot`
verb and PRTSCR-ST keep their wire. The lock discipline is exactly as described (take, work unlocked,
put back — two acquisitions per slice, none across work) and the call sites match this tree:
`prtscr::service` has four callers (`main.rs:1206`, `:1696`, `:3021` the holocron-gated tegra sweep,
`:5957`) and `capture()` has one (`shell.rs:3546`).

`git apply --check` on the prtscr.rs half returned **rc 1 here**, failing at `:391`. The reason matters
more than the failure: **`origin/hw-jetson` and `origin/main` are identical in `prtscr.rs` — zero lines
apart, not the two orin expected — so their patch applies there and their gates are valid.** hw-rmbp is
**+103/−30 ahead** in that same file (`fed449fc` PRTSCR-VOL and reconcile merge `fda50696`), and the
patch's `-391,80 +532,251` rewrites exactly that region.

The symbol census is the whole argument:

| symbol | hw-rmbp | orin's patch |
|---|---|---|
| `mount_capture_target` | 6 | **0** |
| `publish_usb_geometry` | 3 | **0** |
| `BlockSource::Usb` | 2 | **0** |
| `default_writable` | 1 | **0** |

And `Job::begin()` mounts `mount_program_source()` directly — so this is not only a merge hazard, it
**re-opens the defect PRTSCR-VOL closed**: on this board `program_source()` under a `BM_SUBSTITUTED`
verdict returns the read-only Sdhc handle on every call while FRGUARD vetoes the global slot (both
flight-3 proven), so the async capture would refuse forever on the very board SR2's 70 s came from.

**The condition that matters for J1: gate that merge by SYMBOL COUNT, not by a clean apply.** A
take-theirs resolution on a 251-line rewritten `capture()` builds green, checks green, works on the
Orin, and silently deletes the only path by which this board can write a PNG — with nothing red ever
firing. That is the folded-witness failure mode, and it is why the four counts above are written down.

Recorded as ledger rows **B18**, **B19** and **B20**, with SR2's home row in `docs/dev/LEDGER.md`
ticked rather than duplicated.

**One seam recorded both sides:** BSPRUN adds a board leg to `unaos/arroyo` inside `KERNEL_CFG_MATRIX`;
B17 changes the same file on hw-rmbp in disjoint regions (header, one function before the entry point,
one case arm). Union-mergeable at the rmbp landing — flagged to orin so a two-sided arroyo diff at J1
is not a surprise.

## GATE-LEDGER: cross-branch refs, the rule collision pi 7 found

pi 7 brought a live instance of P14 and asked this seat to choose between three exits. The instance,
verified here in the shared object store rather than relayed: `A36 (→ SR2)` at `orin-ledger.md:57` on
hw-jetson `906e3aef`, where SR2 lives on hw-rmbp (`a2ef7279`) and that tree carries **zero `SR` rows in
any of its three ledger files**.

**What made it a rule change and not a one-off: orin did nothing wrong.** `STRUCTURAL_GATES.md`
sanctions the id-cell suffix form, P14 said an unfolded cross-ref stays prose — and an id-suffix
cross-ref cannot be prose without breaking the id convention. The two rules collided and the
*sanctioned* one lost silently: skipped by the old resolver, red under the widened one. Green now, red
later, on a row whose author followed the documented form.

**The split that shipped**, chosen over all three offered exits:

- **Shared ids (`S<n>`, `P<n>`) live in every tree** — measured before relying on it: 27–31 `S` rows on
  main, hw-jetson, hw-pi4 and hw-rmbp alike. An unresolved one is a real dangling ref and stays **RED**.
- **Seat-prefixed ids (`SR`/`SO`/`SP`) are branch-local by construction** — `SR` only on hw-rmbp, `SO`
  only on hw-jetson. An unresolved one is **DEFERRED**: printed every run, counted, named in the summary
  line. Never the silent skip, which is P14's own worst verdict.
- **`UNAOS_LEDGER_STRICT=1` turns every deferral back into a red, and the landing runs strict** — the one
  tree where all three seats' ledgers meet, and therefore the only place a typo can be told apart from a
  legitimate cross-branch reference.

pi 7's preferred exit (skip refs inside the id cell) was declined with its reason: it exempts the id
cell entirely, so a typo there is caught *nowhere, ever*, in exactly the place the contract encourages
refs. **And a discriminator this seat built first and then threw away** — defer only when the prefix has
zero rows in this tree — is sharper and would still catch `→ SR99` locally, but it false-reds in the
partial-fold window (SR1 landed, SR2 not, a ref from a tree that now has one `SR` row), which is
precisely the surprise-mid-landing the change exists to prevent. The rejection is written at the code so
it is not re-proposed.

Four mutation proofs, each run and reverted:

| mutation | result |
|---|---|
| `→ SO99`, default | DEFERRED, **exit 0**, printed by id |
| `→ SO99`, `UNAOS_LEDGER_STRICT=1` | **RED, exit 1** — the landing's setting |
| `→ S999`, default | **RED, exit 1** — shared-id behaviour unchanged |
| the live `→ SR2` already in this ledger | resolves — neither red nor deferred |

That last row is the control, and it is the half a blanket skip would have destroyed: the check still
fires where the target is local. P14 rewritten, `STRUCTURAL_GATES.md`'s invariant updated, ledger row
**B21**.

## Still owed / flagged

- **J1, the rmbp landing** (72 commits) — needs a fleet, so it needs the focus.
- **J2 patch reviews** — none cut yet by orin as of `bd7e13c5` (no `.patch` under
  `docs/dev/evidence/orin16/`). PRTSCR-ASYNC (SR2, this seat owns the row and cannot staff it),
  CRYSTAL-video, DESKFIX-video, ROOTFS-shared, BSPRUN-shared, WINID, MENUBAR2. Standing conditions
  relayed to orin in this seat's first turn, including B16's: **enumerate the callers, do not accept a
  description of them.**
- **J3, SR1's open class** — a gate asserting every Pi-eligible knob has a `K8_FEATS` arm or an
  explicit allowlist. knoboff deliberately does not cover `kernel8.img`, so this class is untouched by
  B17 and is the natural next gate for this seat.
- **J5, B10** — the R19 shut-out register, still never started; a reading task.
- Peter's standing, surfaced once and not chased: **A4**, **B7**, **S27**.
