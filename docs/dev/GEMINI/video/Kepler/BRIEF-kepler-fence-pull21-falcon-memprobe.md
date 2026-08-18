# BRIEF — kepler-fence pull 21: K-GPU-4 milestone 1 — Falcon IMEM/DMEM access probe

Lane: **kepler-fence** — `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`,
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #23, and — binding for
this whole arc — `docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` including
its CLEANROOM POLICY NOTICE. No proprietary blobs, ever; everything we
upload is authored from scratch in this repo.

## The s23 ground truth this pull acts on

The witness rematch REFUTED engine-off as the fence-wall cause (refutation
#7): with PGRAPH enabled, PFIFO still strips VALID/POLL, err=2, identical
signature. The K-GPU-4 arc (from-scratch Falcon microcode) begins. Residual
working theory: PFIFO may require a RUNNING engine — a booted Falcon —
not merely an ungated one.

Blocking fact from s22: post-enable, imemc (0x400180) and dmemc (0x4001C0)
still read BADF1000 — the Falcon memory ports may still be gated. Before
any ucode work can start, we must know whether IMEM/DMEM are writable.
That is this pull, and only this.

## This pull — probe writes to Falcon memory ports, zero execution

After the existing pgraph-enable + settle (unchanged; keep the witness
rematch block as landed — it re-baselines every boot for free):

1. IMEM probe:
   - write IMEMC (0x400180) = 0 | (1<<24) (offset 0, auto-increment, per
     spec §2); readback IMEMC → `:: kepler: falcon imemc wr=01000000 rb=XXXXXXXX ::`
   - write 4 sentinel words to IMEMD (0x400184): DEADBEEF, CAFEF00D,
     12345678, A5A55A5A → marker per word optional, one summary line fine.
   - re-write IMEMC = 0 | (1<<24), read IMEMD back 4× →
     `:: kepler: falcon imem rb w0=XXXXXXXX w1=XXXXXXXX w2=XXXXXXXX w3=XXXXXXXX ::`
2. DMEM probe: same shape via DMEMC (0x4001C0)/DMEMD (0x4001C4) →
   `:: kepler: falcon dmemc wr=01000000 rb=XXXXXXXX ::`
   `:: kepler: falcon dmem rb w0=... w1=... w2=... w3=... ::`
3. NO CPUCTL write, NO BOOTVEC write — zero execution this pull.
4. No restore needed (scratch sentinel words in engine-local memory of an
   idle, never-started Falcon).

Verdict key: sentinel words read back = memory ports live → milestone 2 is
the first real from-scratch ucode (spec §3.1 minimal init + readiness
signal). Reads still BADF1000 / zeros ≠ sentinels = ports gated → next
pull is the secondary-ungating recon (falcon reset/clock registers), NOT
blind writes.

## Gates (DONE = all of these)

New writes are confined to 0x400180/0x400184/0x4001C0/0x4001C4 (Falcon
memory ports of an idle engine — no protection weakened). Full-knob
`UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1
./arroyo check` both arches; default `./arroyo test` + `./arroyo test-arm`
green; builder-path `UNAOS_USBDEBUG=1 <same knobs> ./arroyo esp-x86`;
strings-proof the new `falcon imem`/`falcon dmem` markers in
`target/x86_64_esp/kernel.elf`. Commit ALL docs+code; delete scratch;
`git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-fence-pull21.md`, STATUS: PROPOSED).
