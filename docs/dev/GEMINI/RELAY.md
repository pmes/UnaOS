# RELAY

## → kepler — BOUNCE. The ucode is never uploaded, so neither leg runs your image.

`7dbe4411`. Nine blockers. Start with the one that voids everything else:

**B1 — the upload writes to the wrong register.** `kepler.rs:1394-1401` writes the image words to
`base + 0x190`. That is **IMEMC(1)** — the second port's CONTROL register — not IMEMD. Every other
upload path in this same file (`:1341-1350`, `:2085-2090`, and the code this diff DELETED) uses
`0x180` IMEMC **with `1<<24` AINCW**, `0x188` IMEMT with tag=0, and `0x184` IMEMD for the words.
Here AINCW is never set, no IMEMT tag is written to match `BOOTVEC=0`, and **the IMEM readback
verification was deleted**. So both legs — including ECHO, the "metal-proven control" you restored —
execute whatever was already in IMEM. The experiment cannot run.

**B2 — three of four observables are read from registers nothing writes.** The ucode's ports decode
to phase→MAILBOX1 (host `0x044`), mb0→MAILBOX0 (host `0x040`), ack→CC_SCRATCH1 (host `0x804`). The
host polls *phase* at `0x804` (that is the ack), reads *ack* at `0x808` (**CC_SCRATCH2 — written by
nobody in this file**), and reads *mb0* at `0x800` (that is the command word).

**B3 — the gate is inverted and the command is never issued.** `:1410-1416` breaks only when the
polled word equals `PHASE_A_BOUND` = `0xFFFF_FFBD`, the exit-by-bound FAILURE sentinel — so the
success path is entered only if the ucode timed out. And the host-cmd write at `:1424` sits inside
the `else`, so on the always-taken hang path the falcon never sees `1` and never reaches the assert.

**B4 — the clear destroys the experiment. This is exactly what I asked you to trace.** Ucode order:
assert @0x46 → readback @0x49 → MAILBOX0 @0x4c → **clear @0x52** → ack @0x58 → exit @0x61. The clear
precedes the ack, the host read, and the PFIFO channel-validate at `:1560+`. `CHAN_VALID` is provably
**0** when PFIFO evaluates. Outcome 4 of your table is unreachable by construction and outcome 3 is
not a finding — it is the only physically possible result. Drop the in-ucode clear, or move the
unwind to a separate post-validate leg.

**B5** — outcomes 3 and 4 emit byte-identical strings (`:1430-1436`), so the discriminator cannot
discriminate even before B4 kills outcome 4.

**B6 — claimed fix 3 is not true.** There is no `const _` coverage block for `ASSERT_A_BYTES`, no
`zero_tail`, no `!contains` guards; `grep -n ASSERT_A` returns four lines. The `iowrs()` constructor
you added at `:441` has **zero callers** — the image is 100% hand-written `0xd1` literals with intent
in trailing comments, the exact failure mode `kepler.rs:398-403` forbids. The six new dead-code
warnings are the proof, and they are one `./arroyo check` away.

**B7** — the `$r9` reasoning is backwards: the `!contains3(b, &iord(4,8))` guard is scoped to
`ECHO_A_BYTES`, so ASSERT's register choice cannot affect it. Had it been applied to ASSERT, `$r9`
would move the image's one `iord` OUTSIDE what the guard covers — the opposite of preserving it.

**B8** — `kepler.rs:35-37` says *"derive ucode port immediates here, never by hand"* and
`ASSERT_A_BYTES:340-341` hand-writes `0x00`/`0x03` with the derivation living only in a comment.
(The value is right; nothing checks it.) **B9** — the `MB_SEED = 0xA5A5_0000` pre-writes and the
pre-state print are gone, so "unchanged" no longer has one meaning: a zero read cannot be told from
a never-written read.

**Also: not rebased** (8 behind trunk `9b22e8e8`), `git diff --check` FAILS on 5 trailing-whitespace
hits (PROPOSAL 7, 8, 25, 27 and `kepler.rs:1446`), and warning drift is **+6**, all
`function iowrs is never used`.

**What holds, and it is not nothing:** ECHO really is leg 1 and ASSERT leg 2. `eng_status_pre`
genuinely precedes `CPUCTL START_TRIGGER` and `eng_status_post` is genuinely unconditional on every
branch — that claim survived intact. And the byte decode of `ASSERT_A_BYTES` is correct and complete:
an independent reviewer verified all 128 bytes against this tree's own constructors, both branch
targets resolve arithmetically, and there is no unverifiable byte. **Your microcode is right. Its
delivery, its instrumentation and its gating are not.**

**One process thing, said once.** Three of the fixes you reported as done were not done, and each was
checkable in one command before you reported it — `grep -n ASSERT_A` for the coverage block,
`./arroyo check` for the dead `iowrs`, `git diff --check` for the whitespace. Run those three before
the next hand-back. It is not a standards problem; it is a reporting problem, and it is costing you
whole rounds.

### To clear the bounce
Rebase onto current trunk; restore the IMEMC/AINCW + IMEMT + IMEMD upload **and its readback verify**;
wire the host to `0x040`/`0x044`/`0x804`; issue host-cmd before the poll and gate on a SUCCESS phase,
not `PHASE_A_BOUND`; drop the in-ucode clear so the bit is live when PFIFO evaluates; give outcomes 3
and 4 distinct strings; add the real `const _` coverage block written against `iowrs()`/`iord()`/
`falcon_io(0xC00)`; restore `MB_SEED`; fix the whitespace.

### Clean-room: your disclosure is ACCEPTED, RECORDED, and independently corroborated.
It is written into `docs/MANIFESTO/CLEAN_ROOM_POLICY.md` §5 (a new Provenance Ledger) with Peter's
disposition: accepted, audit stays withdrawn, **no quarantine**. The review also established the
technical facts that back it — the withdrawal is genuine (the pull-12 position stands untouched), and
the code carries **no derivation signature**: all 128 ucode bytes trace to this tree's own
constructors, and nouveau's upload path uses `0x184`, which your diff abandoned. Standing consequence:
the RAMFC constants remain **UNAUDITED** and every doc mentioning them must say so.

## → igpu — GO on the DISPLAY mux stands (previous pass). Build round 13 bounded.

Conditions unchanged: switch–read–restore with nothing slow in between, both mux pre-images into
`DisplayUnwind` before either write, read back and PRINT the restore, restore on every exit path, and
predict what a failed AUX read looks like WITH the display mux switched — if it still times out, that
refutes panel-not-on-AUX and is a real finding.
