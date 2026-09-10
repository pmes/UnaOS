# orin 18 — close report (2026-09-06 20:26Z → 2026-09-07 02:3xZ)

## What landed, and it is pushed
`origin/hw-jetson = 367106ef` (host `ls-remote`, 02:28Z). Six commits above the opening tip:
SHELLRELICS `18af05ab`, VFSROUTE `c8153b4e`, the ledger renumber `ed0c8263` (VFSROUTE's rows S33/S34
renumbered to SO18/SO19 under the frozen-S-id law), the raw-write fixture `f3e64daf`, VOLID
`e668ebde`, and its review notes `367106ef`.

Union gate 9 on that exact tip: `check` 0 · `UNAOS_WC=1 test` 0 (wc-banner=2) · `test-arm 60` 0 ·
`kernel8-test` 0 with MBENCH 119/119 and forbid=0 · fresh-worktree armed-virt 0 (preempt=1 el0=1).

## Ready and NOT folded — orin 19's first act
LAYOUT `exec-orin18-layoutland` **`a6a44eec`** on `367106ef`. Gate 10 GREEN on the first run, no
re-run owed. `/fat` → `/boot`, programs to `/apps`, arroyo stages into `APPS/`. Waits on rmbp 15's
grant; both of their withholding conditions are met and the result went to them at 02:30Z. pi 8
cleared it from pi's side at 01:02Z after re-sweeping. `cdce5129` (C15) folds behind it per rmbp's
B65 ordering condition.

## The defects this round found, in the order that matters
1. **C1 — one card read as two volumes.** `same_volume` compared CONSTRUCTOR STRINGS while
   `sdmmc_root_bind` mounts one card as `"card"` at `/` and `"fat"` at `/fat` — the exact
   configuration render9 flies. Found by rmbp 15 reviewing VFSROUTE. Fixed by VOLID: identity is the
   storage (`volume_fingerprint` + source), never the mount's name.
2. **The test that could not convict it.** The transcript computed its expectation FROM
   `same_volume`, so a wrong answer produced a wrong expectation and the leg agreed with the bug.
   Replaced with a `stat`-based oracle. **The proof is a test that kept passing:** with `same_volume`
   falsified, `vfsroute.ls`/`.cat` still PASS, where before they moved with the bug and stayed green.
3. **B66 — a move between two prefixes of one volume silently mislocated the file and reported
   success.** `rename` handed one backend the OTHER mount's remainder. It rested on its own comment,
   "both remainders are VOLUME-ROOT-relative by construction", which LAYOUT falsified without
   updating the function. Found by rmbp 15. Fixed by translating between mount roots; the stale
   sentence deleted in the same commit.
4. **`sys_open` takes an 8.3 LEAF, not a path.** Staging a fixture blob into `APPS/` broke EL0
   outright (MBENCH 35/119, 7 forbidden). Found BY A GATE, not by reading. The flat EL0 blobs stay in
   the volume root; the loaders carry an explicit `find_app`-then-`find_in_root` order. LEDGER S35.
5. **Quarry's cache cannot invalidate on a CARD volume change** — its stamp is the USB publish
   generation, which `register_sd`/`register_tegra_sd` never advance. rmbp's SR3. Two live defects
   fell out of the registry design pass: `unpublish_usb_geometry` never bumps the generation
   (falsifying the contract documented at the consumer), and `volume_gen`'s x86 twin returns a
   constant.

## Where a claim of mine was refuted, and by what
- **The relics audit's headline was wrong.** "`UNAOS_FBW`/`FBH` have no rebuild trigger" reasoned from
  the absence of a `build.rs`. `rustc` emits `env-dep:` entries for `option_env!` reads and cargo
  consults them; both knobs are there in every aarch64 `.d`. Refuted in-tree. **rmbp's framing is the
  keeper: wrong about the mechanism, right about the smell** — those knobs really do live outside the
  normal machinery, which is what made it plausible.
- **My own census was wrong twice**, both caught by peers: a two-level glob reported ONE `option_env!`
  knob where a recursive one finds TWELVE, and "every `.d` carries it" came from a sample of three.
- **rmbp's ACL-symmetry note is refuted as written**, and I have the red: implementing exactly what
  was proposed took `kernel8-test` to MBENCH 118/119 with 2 forbidden hits. **A rename's destination
  does not exist yet**, so authorizing the destination path asks about a nonexistent object. Preserved
  on `exec-orin18-aclsym` `c8eb4038` as the falsifier any future candidate must survive.
- **pi 8 raised a COUNT blocker and retracted it with commands.** `COUNT <n>` is a FLOOR
  (`mbench.py:21`), so added witnesses can never red it; the real coupling is the mirror image, and a
  rename batch is exactly what removes matching lines.

## Proven and awaiting Peter's ruling
**PANICLOC** (`exec-orin18-panicloc` `b476cea5`). One blank line changes exactly ONE byte — a
`panic::Location` line field at offset 1104296. With `-Z location-detail` in RUSTFLAGS, a line
inserted at the top of ALL 163 kernel sources yields a byte-identical image; without it 438 bytes
differ. Nothing automated depends on the line numbers. The cost is entirely human, on the path that
must work. Two things to weigh: `file,column` leaves the column live, so un-folding the 154 hidden
multi-statement lines is still a re-baselining commit; and every recorded byte-identity baseline in
the tree goes stale on landing.

## Audit reports (bench-side, not in git)
`~/unaos-bench/scratch/orin18/` — `archcore` (605 lines), `archux`, `testrelics`, `battery1/PLAN.md`
(634 lines), `boardsel`, `multiuser`, `stage9dry`, `questions9`. Registry arc brief:
`~/.claude/plans/unaos/wip/volume-identity-arc.md`.

## Process, and it is the part worth keeping
Nine of nine executor-model failures this round were survivable because every executor wrote its
report incrementally to disk; the only real loss was one panel review that rmbp's own review covered.
An ask that arrived as a DIFF cost a peer one verification pass instead of three round-trips, and
both peers said so. Every rule this round added to UNAOS-LAWS was paid for by a defect: ask
OBSERVABILITY first; a true measurement carried past its population is THE failure shape; a gate
protecting rules that never fire is theatre; a rename batch has two opposite obligations; a threshold
is a floor until you read its scorer; scope a scored set by what the harness invokes; and an unstated
invariant shared by two objects is this codebase's defect shape — check it in code, because the
sentence is what rotted all three times.
