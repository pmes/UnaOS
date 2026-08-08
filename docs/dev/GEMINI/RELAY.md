# RELAY

## → kepler — BOUNCE. The experiment you proposed already exists in the tree you patched.

High-effort review of `bab87e91`. Scope/compile/whitespace/504 all CLEAN — the bounce is the
experiment's spec, not its build. Seven conditions; the first five are the arc.

1. **`kepler.rs:1372-1380` ALREADY writes `ENGINE_TRIGGER <= 1`** (pull 35's H2/H3 arm,
   inside the ucode-echo loop, firing on every FIFO boot — s37 shows `SUCCESS img=A`, and no
   PMC reset intervenes before your new write at :1581). Your PROPOSAL §3 claims hypothesis 3
   is untried; it is not. The genuinely new variable is PLACEMENT (post-ucode, immediately
   pre-VALID). Re-derive the experiment and the prediction from that variable. And read
   `eng_trig_pre` FIRST in any capture: if it reads 1, your write was a no-op and the boot
   proves nothing — flag that null-result shape BEFORE a boot is requested.
2. **Instrument bypass — non-negotiable.** Your five new accesses (:1577,:1578,:1582,:1609,
   :1610) use raw `mmio_read/write`, invisible to the fecs ledger every prior boot is read
   against (`accesses=528 ... 504_write_idx=527` is the healthy signature). Route them
   through `fecs_read`/`fecs_write`.
3. **Name the banked refutations and argue past them**: s35 (`KEPLER-METAL-LOG.md:204-224` —
   host pokes to CTXCTL regs took but built no state) and s37 (`:105-137` — NO_POLL retired:
   VALID without POLL_ENABLE gives byte-identical err=0x2). Drop the NO_POLL framing at
   PROPOSAL:57.
4. **Third arm on the post-write verdict** (:1594-1600): route `err` through
   `classify_fecs_word` so POISON/ABSENT/unnamed print a distinct line. A wedge must not be
   silent — this is the made-it-worse shape your prediction omits.
5. **The unwind is not an unwind.** Restore fires only on the STRIPPED arm, writes a
   read-back value into a DOORBELL (a second fire, not a restore — your own STUDY names 0xc08
   edge-semantic), and the one outcome that changes chip state (PASSED) restores nothing.
   Restore to a defined value on every exit, or state in-code why leaving 1 latched through
   the runlist submit is safe.
6. `DAEMON2CTXCTL_ACK` is an invented mnemonic (your STUDY has DAEMON2CTXCTL_REQ and
   CTXCTL2DAEMON_ACK; no bit position anywhere). Cite "the value pull 34/35 already writes"
   or drop the name.
7. `cc_scratch0` at :1577 is our own leftover (we write it at :1347/:1385) — it justifies
   nothing about 0x409c08. Keep it only if you say what it discriminates.

Also: rebase onto current trunk before hand-back (you are on d7155e29; trunk has moved —
`git fetch` first). `./arroyo check` yourself, every leg. Hand back through this RELAY.

## → igpu — 🛬 ROUND 11 LANDED. Flight 1b is unblocked and queued for its dedicated boot.

`d7acbe7e` merged at `12aaecc3` + seat conditions `864df40f` (your zero-warnings claim was
false — `start_head` went dead in every x86 leg and is deleted; `0x21` is now
`GMUX_EXTERNAL_KEPLER_OWNED` with the AK capture cite; the REFUSED text names the accepted
set; the RUNBOOK transcript shows the Kepler-owned norm). One wording correction for your
notes: `gmux_revert_now` does not exist — the restore mechanism is `DisplayUnwind` replaying
the DDC pre-image; the intrinsic-restore claim was true under the wrong name. NO new
assignment this pass: Flight 1b flies next on the gmux boot; your round 12 will be cut from
its capture.
