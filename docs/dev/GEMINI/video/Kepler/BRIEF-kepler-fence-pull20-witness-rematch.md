# BRIEF — kepler-fence pull 20: witness-ladder rematch against a live engine

Lane: **kepler-fence** — `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #22 first.

## The s22 ground truth this pull acts on

Your pull-19 enable TOOK: PMC_ENABLE rb=E011316D, bit 12 stuck. The
all-BADF1200 wall is gone — post-enable the PGRAPH/Falcon block reads
BADF1000 interleaved with real zeros (cpuctl=00000000: Falcon present,
halted, no ucode; imemc/dmemc still gated). The engine now exists on the
pri bus.

The standing fence-wall theory says PFIFO stripped VALID/POLL (err=2)
because the channel's target engine was powered off. That theory is now
testable: **re-run the original channel witness sequence — unchanged —
with PGRAPH enabled.** If PFIFO stops stripping VALID, the fence wall ends
here, with no ucode work at all. If it still strips, the refutation ledger
gains its cleanest entry yet (engine-on, still refused) and K-GPU-4
(cleanroom Falcon ucode) becomes the arc.

## This pull — zero new register writes, resequence only

1. Keep pull 19's enable exactly as landed (it runs first; PGRAPH stays on).
2. After the enable + settle, re-run your established witness sequence
   verbatim — channel/RAMFC setup, runlist submit, VALID/POLL write, the
   err/stat/discriminator reads — exactly as it ran in s7–s10. Do not
   modify the sequence; the ONLY changed variable is the powered engine.
3. Markers: reuse the original witness markers unchanged, plus one framing
   pair so the capture diff is trivial:
   `:: kepler: witness-rematch begin (pgraph on) ::`
   `:: kepler: witness-rematch end err=X stat=X valid=X ::`
   (end-line fields = the same discriminators you already read; keep the
   exact original per-step markers between them.)
4. No restore of PMC bit 12 (enabled is the normal state, per pull 19).

Deliverable: the err=2/strip verdict with the engine on. Either outcome is
decisive.

## Gates (DONE = all of these)

ZERO new register writes (this pull only re-orders/re-enables existing,
already-approved sequence code). Full-knob
`UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1
./arroyo check` both arches; default `./arroyo test` + `./arroyo test-arm`
green; builder-path `UNAOS_USBDEBUG=1 <same knobs> ./arroyo esp-x86`;
strings-proof the two new `witness-rematch` markers in
`target/x86_64_esp/kernel.elf`. Commit ALL docs+code; delete scratch;
`git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-fence-pull20.md`, STATUS: PROPOSED).
