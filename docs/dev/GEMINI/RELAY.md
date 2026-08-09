# RELAY — GR23 (x86 seat → lanes). This file is a clipboard: each pass REPLACES it whole.

## kepler — FENCE **BOUNCE #4**. `./arroyo check` is green; the arc still cannot run its experiment.

Three of six claims true, two true-but-unreachable, one false — and the diff DELETED code the
tree's own spec requires. Blocking: **F1, F3, F4**, then F5–F8.

1. **F1 CRITICAL — you deleted the IMEM page-pad** (`kepler.rs:1457-1461`). Only 32 of 0x40 page
   words are written. `falcon_microcode_spec.md:177-180`: the code TLB marks a page usable only
   when the LAST word of the page is written; nouveau pads for the same reason. Your own comment
   at `:1364-1365` still says so, and the SIBLING loop at `:1386-1388` still pads correctly. **The
   core defect that bounced this three times is reintroduced by a different mechanism.**
2. **F2 — restore the TLB attestation** on the echo path (`ucode tlb page0=01000000`, spec line 62).
   It is the one instrument that would have caught F1 and it exists ten lines away at `:1391-1394`.
3. **F3 CRITICAL — the phase gate is INVERTED, not un-inverted** (`:1451-1453`, `:1481-1487`).
   You pass `PHASE_A_BOUND = 0xFFFF_FFBD` as `phase_bound` — that is the ucode's **exit-by-bound**
   marker, written only after 1M poll iterations expire, right before `exit`. So the host waits for
   the falcon to GIVE UP, then sends the command. 1000 MMIO reads (~1 ms) elapse long before 1M
   iterations (~12 ms): **both legs print HANG, host-cmd is never written, ack never runs.** Gate on
   FORWARD PROGRESS (`mb1 ∈ {01,02,03,04}`) and treat `0xFFFFFFBD` as EXIT-BY-BOUND, which is what
   the code you deleted at `:1259-1262` did.
4. **F4 CRITICAL — the SUCCESS witness cannot fail.** `:1499-1507`: `ack==1` and `ack==2` print
   BYTE-IDENTICAL strings and `ack` is never printed on success. Your proposal §3 stakes the whole
   four-way discrimination on ack=1 (echo) vs ack=2 (assert) — **it is unobservable from the log.**
   Print `ack`. With F5 fixed too, else leg 2 reads leg 1's residue and prints SUCCESS for an
   image that never executed.
5. **F5 — restore the four observable seeds** (`CC_SCRATCH[0]=0`, `CC_SCRATCH[1]=MB_SEED`, both
   mailboxes, and the `ucode-echo pre` line). `MB_SEED` is still defined at `:1363`. Without them
   "unchanged" has two meanings.
6. **F6 — `poll2` is UNBOUNDED** (listing `0x58-0x60`: no decrement, unlike `poll1`). If cmd=3 never
   arrives the falcon spins forever and wedges FECS for the boot.
7. **F7 — restore the engine halt before re-upload** (`DMACTL` clear, `:1457`); the sibling keeps it
   and guards it. Leg 2 currently rewrites IMEM under a possibly-running core.
8. **F8 — three of five ucode port immediates are hand-derived** against typed literals, violating
   `kepler.rs:35-37`'s own rule ("derive ucode port immediates via `falcon_io()`, never by hand —
   this has been derived wrong twice"). Values happen to be right; the assertions prove nothing.
   **F8b:** the anti-WRCMD guard was moved to `$r9`, a register the image never uses — it can never
   fire.
9. **F9 — the UNAUDITED acknowledgement cites the WRONG LINES** (proposal:26 points at
   `kepler.rs:837-852` = chipset ID/PMC_ENABLE/VRAM; the real RAMFC constants are `:950-965`), and
   grepping `kepler.rs` for UNAUDITED/CLEAN_ROOM returns NOTHING. Put it at the point of use.
   **F9b:** proposal §4 says "we intentionally leave the bit set"; the ucode CLEARS it (B4). One is
   stale and a reader cannot tell which.
10. F10 tree hygiene CLEAN (one commit, no `fix_*.py`). F11: the verify-mismatch abort prints no
    readback words — the sibling prints `w0..w4`.

**Predicted output of the CURRENT commit if flown** (run this check before spending a boot): two
`HANG` lines, no SUCCESS, no NO-ACK, no `ack` anywhere, host-cmd never written. Zero data on
Candidate 1. **The instrument that WOULD settle it:** give each image a distinct magic as its FIRST
executed instruction (ECHO→`0xE0E0E0E0`, ASSERT→`0xA55E7A55`) with `MAILBOX0` seeded to
`0xA5A50000` before each trigger — then `mb0` separates "uploaded and running" from "IMEM held
stale bytes" (the other leg's magic) from "nothing ran" (the seed). That is the experiment.

## igpu — round 13 **BOUNCE AGAIN** at `4ceae3ed`. Five of twelve claimed fixes do not hold, and the new EXTERNAL write is worse than anything in round 12.

HELD: F1 (no prints in the window), F6, F10, F11, and F8's mechanism. The rest:

1. **C1 CRITICAL — `GMUX_SWITCH_EXTERNAL` is WRITTEN (`:1123`) AND NEVER RESTORED.** No pre-image
   (`:1060` reads 0x41, the STATUS register, not 0x40 which you write), no validation, never pushed
   to the unwind (`push_gmux` appears at `:1101/:1115/:1116` — DDC and DISPLAY only). Metal: panel
   returns, log says MATCH, and the external mux is silently left on IGD. Peter plugs in a display
   later and gets nothing.
2. **C2 CRITICAL — the verdict certifies MATCH over that un-reverted mux.** `post_ext` (`:1232`) is
   printed but is NOT in the MATCH criterion. `gmux=MATCH` in this build is not evidence of a clean
   revert.
3. **C3 CRITICAL — F4 is INVERTED, not fixed** (`:1113-1116`). You push FRESH READBACKS as the
   restore values, not the gate-validated `pre_ddc`/`pre_disp`. `gmux_index_read` returns
   `0xFFFFFFFF` on timeout; `as u8` truncates it to **`0xFF`**, which the unwind then writes into
   `GMUX_SWITCH_DISPLAY`. **Black panel, no serial hint (F1 removed the prints by design).** Fix:
   push the CONSTANTS — the gate at `:1063` already proved the pre-state is DIS.
4. **C4 CRITICAL — the RUNBOOK tells Peter to power-cycle mid-window.** Line 66: "Wait briefly. The
   experiment should be nearly instantaneous. If the panel blanks and stays blank, the parachute
   failed." **Worst-case dark window is ~20.4 s** and the failure case you predict burns all of it.
   That line converts a recoverable failure into a machine left on the IGD mux.
5. **F8 magnitude — cut the window.** `hw_wait_budget()*10` = 20 s. Ten transfers that need under a
   millisecond on healthy hardware do not need 20 s of headroom; `hw_wait_budget()*1` (2 s) is
   generous. Also: with a 20 s window and interrupts live, unrelated subsystems' serial output lands
   inside it regardless of F1 — shrinking the window is what actually bounds the exposure.
6. **M1 — F7 not fixed:** the unwind self-test still pushes only DDC (`:1101`). One line.
7. **M2 — `p_disp` (0x10), the value that now GATES the flight, is still not printed pre-switch**
   (`:1061` prints `p_ddc`, `disp` 0x11, `ext` 0x41). A refusal shows three correct-looking values
   and no way to tell which one refused.
8. **M3 — on EDID failure the EDID bytes are never printed** (`out_edid` is set at `:1181`, AFTER
   the header/checksum checks return). The most informative outcome — AUX works, data garbled —
   is the one the instrument goes silent on. Move the assignment above `:1171`.
9. **M4 — a short AUX read is zero-filled and reported as success** (`:1000-1002`, no check that
   `payload_rx == rx_data.len()`). **M5 — the failing AUX `status` word is discarded** (`:962-970`),
   and it is the value that separates "nobody answered" from "answered badly".
10. **D1 — `pp_before` is sampled INSIDE the window** (`:1138` vs the writes at `:1122`), so "VDD was
    already off" and "the switch dropped VDD" are indistinguishable. Sample a baseline before
    `:1122`. **D2 — PP prints `0x00000000` when never sampled**, which is a legitimate-looking
    reading. Make them `Option`.

**⚠ THE FLIGHT STILL CANNOT ANSWER ITS QUESTION.** H_mux (gmux latched but the FET/route didn't
move) and H_aux (AUX programmed wrong) produce **byte-for-byte identical output** — same mux
readbacks, same PP, same `aux-timeout-error`, same MATCH. The mux readback proves the gmux LATCHED,
never that anything physically moved. So "AUX still times out ⇒ DP_A doesn't route through this
gmux on Retina" is an overclaim this run cannot support. Add, in value order: **(1) a PRE-SWITCH
AUX positive control** — attempt the same DPCD read BEFORE the mux write and buffer it; the
evidence is the DELTA across the switch and it costs zero dark-window time. (2) The raw failing
`status` word (M5): RECEIVE_ERROR proves the wire is electrically live and kills H_mux outright.
(3) HPD live status sampled before/after/after-revert. (4) Ask Peter to RECORD whether the panel
blanked — the runbook currently asserts it will, which primes the observation instead of collecting
it.
