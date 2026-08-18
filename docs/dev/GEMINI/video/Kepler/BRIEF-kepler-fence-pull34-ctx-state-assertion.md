STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-kepler-fence-pull34.md`, this directory)

# BRIEF — kepler-fence pull 34: context-state assertion (K-GPU-4 Milestone 6)

Coordinator-authored (2026-07-30, GR9). Predecessor: pull 33, LANDED at merge
`9621bbc9`, acked on metal at sitting #37, confirmed repeatable at sitting #42.

## Where it stands

Pull 33 opened the constructive era: **our own microcode answered a command.**

```
:: kepler: ucode-echo pre      CC_SCRATCH[0]=00000000 CC_SCRATCH[1]=00000000 ::
:: kepler: ucode-echo host-cmd CC_SCRATCH[0]=00000001 ::
:: kepler: ucode-echo host-ack CC_SCRATCH[1]=00000001 iters=0 ::
:: kepler: ucode-echo SUCCESS  img=A ::
```

Image **A** — the derived indexed ports `I[0x20000]`/`I[0x20100]` — acked on the first
poll. That settles two things: the host↔FECS command loop works, and the indexed IO
scheme `host X → falcon (X & 0xffc) << 6` is confirmed for a **second** register family
beyond the s29 mailbox proof.

Ten-plus sittings of elimination have established that PFIFO channel validation depends
on state built by the FECS context-switch microcode, and nothing else we have been able
to reach. That is the actor this pull goes after.

## What pull 34 must cover

**1. ⛔ MUST-FIX — the unbounded echo loop (standing defect carried out of land review).**
The echo loop as landed branches back to the `iord` **forever**, contrary to the
bound-every-loop law (`falcon_microcode_spec.md` §5.1) and to the bounded discipline
pull 27 used. It caused no harm at s37 (the boot stayed healthy through 449+ SMC samples)
and none at s42, but a **host-commandable exit** was explicitly owed to this pull. Deliver
it. An unbounded loop that has not yet hurt us is a loop we have not yet been unlucky with.

**2. Ground-truth recon probe FIRST — read-only, before any write.**
`STUDY-fecs-ctx-init.md` names the handshake surface at FECS base `0x409000`: `CHAN_CUR`
(`0xb00`), `CHAN_NEXT` (`0xb04`), `ENGINE_STATUS` (`0xc00`), `ENGINE_TRIGGER` (`0xc08`),
`WRCMD_DATA`/`WRCMD_CMD` (`0x500`/`0x504`), `CC_SCRATCH` (`0x800`+). Probe their reset
values and print them raw before proposing a single write.

This ordering is not ceremony. The poison history on this part is **per-offset** — `0x409504`
alone is convicted, and the subunit-gating theory that would have explained it as a class
was REFUTED at s33 (`PIBUS_MMIO_HUB_ENABLE1=FFF9F4B0`, bit 4 already set) and s34 (all five
remaining offsets read real zeros). So each new offset's readability is its own question,
answerable only by reading it. A hypothesis tested against an offset that does not read is
not tested.

**3. The assertion itself — ranked minimal hypotheses.** `STUDY-fecs-ctx-init.md` gives
four, ordered by minimality; take them in that order and make each independently
falsifiable on one boot:
   1. `CHAN_CUR` (`0xb00`) — write the channel ID (± a VALID high bit), mimicking a switch.
   2. `ENGINE_STATUS` (`0xc00`) `CHAN_VALID` bit 1.
   3. The `CC_SCRATCH` / `ENGINE_TRIGGER` (`0xc08`) host handshake completing.
   4. `DMACTL` `REQUIRE_CTX` interacting with `CHAN_CUR`.

   The success criterion is unambiguous and pre-declared: **PFIFO channel validation stops
   refusing** — `err=2` goes away — or it does not. State, per hypothesis, what the witness
   will print in each case *before* the boot.

**4. Port derivation + A/B fallback for any new register family.** Every falcon-internal
port must be derived by `(X & 0xffc) << 6` and, where the derivation is not already
metal-confirmed for that family, shipped as an **A/B pair** with the attempt labelled in the
marker. That pattern has now paid for itself twice (pull 25's port question, pull 33's
CC_SCRATCH ports — where the proposal shipped host offsets as falcon port indices and the
A/B settled it in one boot). It costs one image and buys a settled fact.

**5. The falcon-side read — the first fact only our microcode could obtain.**
Carried forward from the pull-34 invitation and now cheap, because the command loop works.
`0x409504` is convicted **host-side**; the falcon may own it legitimately, and the falcon can
reach unit space the host cannot. Have the ucode **read** a ctx-relevant register from inside
the falcon and report it back through the echo loop. Whatever it returns is knowledge no
host-side probe in this entire series could have produced — and if it returns something other
than poison, the per-offset poison model gains its first mechanism rather than just its
boundary.

## Leads that are DEAD — do not re-derive them

Each cost real boots. Re-proposing any of these is the failure this section exists to prevent.

- **The poll area** (pulls 11/12) — refuted. Stays dead.
- **`err=2` as "NO_POLL"** — the chip's own error name is a **red herring**, proven at s37:
  VALID written *without* POLL_ENABLE produces a byte-identical `err=00000002`. Twenty-eight
  sittings honored a reason name that does not describe its own precondition. What survives is
  only: err=2 means "channel table validate refused", and nothing finer.
- **CTXCTL subunit gating** — refuted at s33/s34 (above).
- **`USERD_SNOOP` (`0x2a1c`) as a global knob** — writes read back as zero; candidate A was
  cleanly refuted at sitting #10 with no residue.

## Laws for this pull

- **Cleanroom.** `envytools` hwdocs and rnndb are permitted as **facts with citation**. No GPL
  code bodies, ever. Any offset that arrives without a citation must be labelled as
  empirically probed, honestly. (Standing debt in this lane: `kepler.rs:~465`'s EVO
  core-channel offsets still carry a "derived from nouveau/gf119.c" comment from pull 3 —
  it must become an rnndb citation or an honest probed note before merge. Not this pull's
  scope, but do not add a second one.)
- **Bound every loop** (`falcon_microcode_spec.md` §5.1). See must-fix 1.
- **Gate: `./arroyo check`, both arches, only.** Per Peter this sitting, the QEMU suites
  (`test`, `test-fat`) are dropped — do NOT run them. They cannot reach this path in any
  case; metal is the verdict.
- **Verify the symbols are IN the artifact**, not merely that the build was green — the
  s42 INSTGUI lesson (a knob added only to `arroyo` is invisible to `builder/`, so it ships
  disabled while every check passes). `strings` the artifact before staging.
- **Read before write; print raw before decoding**; state what each witness reads in the
  healthy-but-idle case, so no counter can report its own baseline as a result.

## Owed

Metal: the next Kepler sitting. Report to the coordinator seat (x86/GR9) as
`PROPOSAL-kepler-fence-pull34.md` in this directory.
