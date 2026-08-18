STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-kepler-fence-pull35.md`, this directory)

# BRIEF — kepler-fence pull 35: fix the poisoning read, then make FECS access itself a witness

Coordinator-authored (2026-07-30, GR10). Predecessor: pull 34, LANDED in `46f8f37e`.

## ⛔ Read this before anything else: pull 34 has NOT been on metal, and as it stands it would void its own boot

The s58 sitting has not happened. Pull 34's code is in the tree and has never executed on
silicon. **Pull 35 exists to make s58 admissible.** Nothing in this brief responds to a metal
result, because there is no metal result to respond to; do not write a proposal that pretends
otherwise.

Two defects in the as-landed pull 34 must be fixed before a card is cut. Both are ordering
defects, not logic defects — the code does what it says, at the wrong point in the boot.

### MUST-FIX 1 ⛔ — the head recon probe reads the poison offset, first, before every FECS witness in the boot

`unaos/crates/kernel/src/drivers/gpu/kepler.rs:493`:

```rust
let recon_wrcmd_cmd = mmio_read(bar0, 0x409504);
```

That offset is `WRCMD_CMD`. This file's own ordering contract, 700 lines further down at
`kepler.rs:1229–1256`, states the rule that read breaks:

> `0x409504` (WRCMD_CMD) is the poison offset: the first ACCESS to it faults and wedges every
> subsequent read in the FECS unit for the rest of the boot (s31 discovered, s32 confirmed with
> its own control frame, s34 convicted by elimination — `falcon_microcode_spec.md` §5.4).

The terminal poke is placed last, and carries `NO READBACK`, for exactly this reason. Pull 34
then put an unconditional **read** of the same offset at the *head* of `init()` — before the
ucode is loaded, before the echo, before the bind probe, before every FECS witness the pull was
built to obtain.

**What this costs if it ships.** Take the poison model as it stands and read the boot forward
from line 493. Clean: the five reads at `:488–492` (they precede it). Poisoned: `ucode-echo pre`,
`host-cmd`, `host-ack`, `post-witness`, `final`, the ucode verify readback, `bind-pre`,
`bind CHAN_CUR`, `bind CHAN_NEXT`, `bind-post`, `recon-pre`, `recon-post`. Every one of those
would print a fault value, and the capture would read as **the echo regressing** — the exact
false conclusion this lane has spent forty sittings learning to refuse. A sitting burned is bad;
a sitting burned while manufacturing a wrong finding is worse.

**The fix.** Remove `0x409504` from the head recon probe. It buys nothing: s31/s32/s34 already
established that reading it faults, and the *write* question is already the terminal poke's, at
the end of the boot where it belongs. Keep the other five reads — `0x409500` (`WRCMD_DATA`) is a
different offset and the per-offset poison model says each one is its own question.

**If you want to challenge the poison model, that is legitimate — but it is its own experiment.**
It would be an A/B across two images differing only in whether the head read is present, with the
FECS witnesses compared line by line. It **cannot ride the same image as the echo work**, because
it would void it. Do not bury a model test inside a recon probe. If you propose the A/B, propose
it as a separate image pair and say what each side is predicted to print.

### MUST-FIX 2 ⛔ — the H2/H3 writes landed unconditional, unapproved, and ahead of the state they perturb

`kepler.rs:500` and `:504` write `ENGINE_STATUS <= 2` (CHAN_VALID) and `ENGINE_TRIGGER <= 1`,
unconditionally, at the head of `init()`. The land review said plainly: *"H2/H3 writes are
unapproved… they are not approved until the proposal is ruled on."* They shipped anyway, ungated.

Beyond approval, the placement is wrong on its own terms. These are writes into the FECS unit —
the unit pull 28 put under a standing ban on unproven writes, an exemption Peter granted **once**,
for the terminal poke, with the note that it does not generalise. They execute before the ucode is
loaded and before `bind-pre` reads `ENGINE_STATUS` at `:1067`, so the bind probe no longer measures
the reset state of the register: it measures what pull 34 wrote into it. The one thing `bind-pre`
existed to establish is now unobtainable on any boot that runs this code.

**What pull 35 owes here:** state what `ENGINE_STATUS` and `ENGINE_TRIGGER` read in the untouched
case (you have the recon values from `:490–491` — say what they are expected to be and why), then
either move the writes adjacent to the assertion they belong to, or ship the with/without pair as
the lane's established **A/B image** idiom with the attempt labelled in the marker. Not a build
knob invisible to `builder/`; the A/B pattern has paid for itself twice in this lane (pull 25's
port question, pull 33's CC_SCRATCH ports) and it is one image for a settled fact.

## Milestone 2 — the FECS access ledger: make "was the unit poisoned, and by what" a reading

Must-fix 1 is a comment-enforced contract that a later edit walked straight through. The contract
should not be a comment. Build the instrument that makes this class of defect impossible to ship,
and that answers on every future boot the question we are currently answering by inference.

1. **Route every FECS-unit access through one pair of functions** — `fecs_read(bar0, off)` /
   `fecs_write(bar0, off, val)` covering `0x409000–0x409FFF`. Convert all existing call sites.
   A direct `mmio_read`/`mmio_write` into that window becomes the thing a reviewer can grep for.
2. **Ledger, in order:** access count, the offset of the **first** access this boot, whether
   `0x409504` has been accessed and at which index, and the index at each named witness. One
   compact line, not a per-access spew — the FTDI ring budget is real (see the gated-off dense
   recon at `:953`).
3. **Baseline discipline, non-negotiable** (`docs/dev/OS/07_USB_STORAGE/usb_xhci.md` §16.9's law,
   which binds every lane): state, in the proposal and in the witness comment, what the ledger
   prints on a **healthy** boot, what it prints when the poisoning access **has** occurred, and
   show those differ. A ledger whose two readings look alike cannot falsify anything.

This is the milestone that pays forward. Had it existed, must-fix 1 would have been a line in a
capture rather than a defect found by reading source.

## Milestone 3 — the assertion, with a decision table written before the boot

Pull 34's ranked hypotheses stand. 1 (`CHAN_CUR` write) and 2 (`ENGINE_STATUS` CHAN_VALID) are in
the tree; what remains untried is:

3. The `CC_SCRATCH` / `ENGINE_TRIGGER` (`0xc08`) **host handshake completing** — not the trigger
   write alone, the full sequence with the falcon's response observed.
4. `DMACTL` `REQUIRE_CTX` interacting with `CHAN_CUR`.

Success criterion is unchanged and pre-declared: **PFIFO channel validation stops refusing** —
`err=2` goes away — or it does not. Per hypothesis, write down what each witness prints under
"worked", "did not work", and "instrument did not run", **before** the image is built.

**Preserve, do not re-derive:** pull 34's falcon-side read of `0x409504` from inside the falcon
(port `(0x504 & 0xffc) << 6 = 0x14100`, A/B pair, compile-time asserted at `kepler.rs:288`) has
never executed on metal. It is the first fact only our own microcode could obtain. Carry it
forward unchanged and report it.

## Leads that are DEAD — do not re-derive them

Each cost real boots.

- **The poll area** (pulls 11/12) — refuted. Stays dead.
- **`err=2` as "NO_POLL"** — the chip's own error name is a red herring, proven at s37: VALID
  written *without* POLL_ENABLE produces a byte-identical `err=00000002`. What survives is only:
  err=2 means "channel table validate refused", and nothing finer.
- **CTXCTL subunit gating** — refuted at s33 (`PIBUS_MMIO_HUB_ENABLE1=FFF9F4B0`, bit 4 already
  set) and s34 (all five remaining offsets read real zeros).
- **`USERD_SNOOP` (`0x2a1c`) as a global knob** — writes read back as zero; refuted at sitting #10.

## Laws for this pull

- **Cleanroom.** `envytools` hwdocs and rnndb are permitted as **facts with citation**. No GPL code
  bodies, ever. An offset without a citation must be labelled as empirically probed, honestly.
  (Standing lane debt: `kepler.rs:~465`'s EVO core-channel offsets still carry a "derived from
  nouveau/gf119.c" comment from pull 3. Not this pull's scope — do not add a second one.)
- **Bound every loop** (`falcon_microcode_spec.md` §5.1).
- **Read before write; print raw before decoding**; state what each witness reads in the
  healthy-but-idle case, so no counter can report its own baseline as a result.
- **Gate: `./arroyo check`, both arches, ONLY.** Do NOT run `./arroyo test`, `test-fat`, or any
  QEMU target — QEMU has no Falcon and cannot reach this path, and the runs cost money. Metal is
  the verdict.
- **Verify the symbols are IN the artifact** as `builder/` produces it (the `esp-x86` media
  artifact), not in a `.rlib` — the s42 INSTGUI lesson: a knob known to `arroyo` and unknown to
  `builder/` ships the feature DISABLED with every check green. `strings` it before staging.
- **Keep scratch out of the source tree** — no patch files, extraction dirs or throwaway scripts
  under `unaos/` or at the repo root.
- **PROPOSAL FIRST.** Until this brief's proposal is ruled on, the only file you create is
  `PROPOSAL-kepler-fence-pull35.md`. Do not modify anything under `unaos/crates/kernel/src/`.
  The one exception, because it blocks the card: if you want must-fix 1 landed ahead of the rest,
  say so in the proposal and it will be ruled on first.

## Owed

Metal: the next Kepler sitting (s58). Report to the coordinator seat (x86/GR10) as
`PROPOSAL-kepler-fence-pull35.md` in this directory.
