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

## → kepler-fence

Brief: `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-fence-pull34-ctx-state-assertion.md`

Fix these in what you already wrote (nothing is being thrown away):

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

## → iGPU

Brief: `docs/dev/GEMINI/video/iGUI/BRIEF-igpu-pull6-power-on-battery.md`

**Keep as-is:** your M2 verdict — *"the cheap win does not exist"*, full iGPU bring-up
enumerated, no gmux write proposed — is right. M3 correctly writes nothing.

Fix these:

1. **`:: PWR: ::` prints only `avg_draw`.** A bare mean is the exact error we just disproved on
   the USB side, where a believed "~9 ms" figure was one outlier dominating a small-n mean and
   the honest number was ~1 ms. **Print raw counters — samples, sum, min, max** — and let the
   reader divide.
2. **The signed accumulator averages across a state change.** `state` is read from the last
   sample at rollup, so a window spanning a plug-in event is labelled with its final state while
   containing both charging and discharging samples. Reset the accumulator when `ac_derived`
   changes, or accumulate per state.
3. **`AcDerived::Unknown` samples still enter the average.** Exclude and count separately.
4. **The sign convention is inherited from a code comment, not derived.** Either derive it on
   metal or label the witness as carrying an inherited assumption.
5. **State the healthy-but-idle reading in the code** — what `:: PWR: ::` prints when the
   mechanism is healthy but nothing is happening.
6. **Remove** `unaos/crates/kernel/src/drivers/smc_pwr.patch`.

M1 stays read-only. No gmux write without Peter's explicit go.

---

## → kepler-display

Idle, no brief this round.
