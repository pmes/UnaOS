# PETER'S RELAY SHEET — what to post into each Gemini chat, right now. Nothing else.
# Overwritten every round. History lives in the briefs and the metal log, not here.
# (2026-07-30, GR9)

## → ALL sessions

We are on the same Linux box now. Read your brief directly from the tree.

**Stop committing per pull.** The old "Commit ALL docs+code, no push, report PUSH OWED: n"
is withdrawn. Leave your work as files; we take one commit at round end.

⛔ **PROPOSAL FIRST — no kernel source edits before approval.** Dropping the commit step did
not merge the phases. The loop is: **you propose → coordinator reviews → THEN you implement.**
Until approved, the only file you create is `PROPOSAL-<lane>-pull<N>.md`. Do not modify
anything under `unaos/crates/kernel/src/`. Your brief's `STATUS: BRIEF — awaiting Gemini
proposal` header is the authority; if this sheet ever contradicts it, the brief wins.

**Gate: `./arroyo check`, both arches, only.** Do NOT run `./arroyo test` or `test-fat` —
QEMU has no gmux, no SMC, no panel, no Falcon. Metal is the verdict.

**Verify your feature's symbols are in `kernel.elf` as `builder/` produces it** — not in a
`.rlib`. A knob added only to `arroyo` is invisible to `builder/`, so the feature ships
disabled while every check passes.

**Keep scratch out of the source tree** — no patch files, extraction dirs or throwaway
scripts under `unaos/` or at the repo root.

---

## → kepler-fence — CODE REVIEW: **BLOCKED.** Two defects, fix before anything else.

**1. ⛔ YOU DELETED 47 TRACKED LANDING REPORTS.** Your worktree shows the entire `review/`
directory deleted — `fox-metal-r22s2`, `unaos-flight-recorder`, `unaos-install*`, `unaos-orin-*`,
`unaos-v3d*`, 4672 deletions. That is the project's historical record. Nothing in your brief or
this sheet asked for it. They are unstaged working-tree deletions, so:
```
git checkout -- review/
```
Do that first. Nothing from this worktree gets committed until it is done.
Also remove, again: `patch_kepler.py`, `patch_tests.py`, `unaos/out.txt`.

**2. ⛔ THE RECON PROBE READS THE WRONG REGISTER AND MISLABELS IT.**
```rust
let recon_chan_cur = mmio_read(bar0, 0x409000);
serial_println!(":: kepler: recon CHAN_CUR={:08X} ::", recon_chan_cur);
```
`CHAN_CUR` is FECS base **+ 0xb00** = `0x409b00`. `0x409000` is the base itself. That witness
line prints a false statement — the exact instrument-lie class that has cost this project six
separate findings. And the other five registers your approved proposal promised are not read at
all: `CHAN_NEXT` 0xb04, `ENGINE_STATUS` 0xc00, `ENGINE_TRIGGER` 0xc08, `WRCMD_DATA` 0x500,
`WRCMD_CMD` 0x504. "Recon probe before any write" is the one thing that had to land first,
because the poison on this part is **per-offset** — each offset's readability is its own
question, and an offset you never read is a question you never asked.

**GOOD — keep it:** the bound is correctly implemented. `ECHO_BOUND`, `sethi $r5, 0xfff0`, and
`echo_loop_is_bounded` asserting both exits are a real `exit` (`f8 02`) "not a fall-off-the-page".
Your proposal's prose said the loop runs "indefinitely"; your code is bounded. The code was right
and the prose was wrong — fix the prose.

### Still owed from the proposal verdict (unchanged)

Branch math shown, milestone relabelled, falcon port derivation right (`0x504 << 6 = 0x14100`). Good.

1. ⛔ **THE BOUND CONTRADICTS ITSELF.** §1 says the ucode "will now loop **indefinitely** waiting
   for commands"; §2's branch math shows an `r5` decrement with `bra ne, poll` — a counter. Both
   cannot be true. The must-fix is to **bound** the loop; a host-commandable exit is necessary but
   NOT sufficient, because if the host never sends the sentinel (write doesn't land, host wedges)
   the falcon spins forever — which is exactly what `falcon_microcode_spec.md` §5.1 and pull 27's
   discipline forbid. State the iteration bound, its value, and what the ucode does at expiry.
2. **The strings check is still wrong.** You propose `./arroyo build` "or directly in `builder/`".
   `./arroyo build` is **not** `builder/`, and that divergence IS the INSTGUI bug — a knob known to
   `arroyo` and unknown to `builder/` ships the feature DISABLED with every gate green. Verify
   against `kernel.elf` **as `builder/` produces it** (i.e. the `esp-x86` media artifact).
3. **Cite the branch-offset base.** You compute `target − branch_address`. Say whether the ISA
   measures the displacement from the branch instruction or from the following one, and cite it.
   An unstated base is a 3-byte error, and it is the same class that bit pull 25 and pull 33 on
   port encodings — both caught only by the A/B fallback you are already shipping here.

Everything else approved as proposed: recon probe before any write, H2/H3 with readback, the
falcon-side read of `0x409504` from inside the falcon with its A/B pair, and the exit sentinel.

Also still owed from the earlier review:

1. **Milestone mislabel** — your report says "K-GPU-4 Milestone 2"; the brief scopes pull 34 as
   **Milestone 6, context-state assertion**. Say which you built.
2. **Strings were checked in `.rlib`, not the artifact.** Re-verify against `kernel.elf`.
3. **The 128-byte padding moved your offsets** (`0x25` → `0x2c`). Your claim that executing code
   stays inside the first 92 bytes is unproven in the report — show where every relative branch
   lands after padding.
4. **H2/H3 writes are unapproved.** Ordering is right (recon reads do precede them). Leave them;
   they are not approved until the proposal is ruled on.
5. **Remove from the tree:** `patch_ucode.py`, `patch_ucode.out`, `unaos/test_diff.py`,
   `unaos/tmp_extract/`, top-level `review/`. Landing reports go in
   `docs/dev/GEMINI/video/Kepler/`.

Then the pull itself, in order: **(a)** bound the echo — host-commandable exit, MUST-FIX, owed
since pull 33; **(b)** read-only recon probe of the FECS handshake surface at `0x409000` before
any write; **(c)** H2/H3 with readback after every write; **(d)** the falcon-side read — have the
ucode read a ctx-relevant register from inside the falcon and report it through the echo.
`0x409504` is convicted host-side only; the falcon may own it legitimately.

Any new register family: derive ports by `(X & 0xffc) << 6` **and** ship an A/B pair.

---

## → iGPU — CODE REVIEW: **APPROVED — cleared to go into the next card image.**

All six findings are implemented in `smc.rs`: `samples`/`unknown`/`sum`/`min`/`max` instead of a
bare mean; the window flushed when `ac_derived` changes so charging and discharging never mix;
`Unknown` binned separately; and `(sign convention: inherited assumption)` printed in the witness
line itself.

Checked specifically, because it would have been an instrument lie: `min`/`max` are **seeded from
the first sample**, not from 0. An all-negative discharge window therefore cannot report a `max`
of 0 mW that never occurred. Correct.

Remaining: remove `unaos/crates/kernel/src/drivers/smc_pwr.patch`, and get the baseline on metal
(unplugged — a terminal current on AC is charging, not system draw).

Original proposal verdict, for reference:

All six review findings are fixed: raw counters (samples/sum/min/max) instead of a bare mean;
the window flushed on an `ac_derived` change so charging and discharging are never averaged
together; `Unknown` excluded into its own bin; the sign convention labelled as an inherited
assumption **in the witness text itself**; a healthy-but-idle example line; and the M2/M3 scope
kept honest.

Implement M1 only. It stays read-only — no gmux, display-engine or SMC writes. M2 stays a
scoping document and M3 stays a plan; no gmux write without Peter's explicit go, given
separately from this approval.

One housekeeping item: remove `unaos/crates/kernel/src/drivers/smc_pwr.patch` from the tree.

Your M2 verdict — *"the cheap win does not exist"* — is right and stands as the lane's answer.

---

## → kepler-display

Idle, no brief this round.
