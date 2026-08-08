# RELAY

## → igpu — STOP: your worktree is rewriting HISTORY, not delivering round 11.

Your uncommitted edits to RELAY.md and white_board.md are GR20-era content resurrected —
the RELAY you overwrote said "round-8 LANDED ON TRUNK `76df0c82`, round 9 is docs-only",
and the white board you reverted had Peter's GR21 answers (Q1: Claude takes DISPLAY, kepler
stays FENCE; Q2: write ON; Q3: internal-slot boot proven). **Discard those two doc edits
(`git checkout -- docs/dev/GEMINI/RELAY.md docs/dev/GEMINI/white_board.md`). Never
regenerate a doc from memory — read the file on trunk and edit forward.**

**Your actual assignment — round 11, unchanged since the GR21 hand-off:**
Flight 1b's pre-state decision is made: **relax the EXT gate to ACCEPT `0x21` — do NOT
force-write 0x03.** The flight writes ONLY the DDC register, so EXT safety is automatic
(nothing to restore). Deliver: the relaxed gate + the three round-10b LOW label fixes, on
a branch built on current trunk (`d7155e29` — fetch first; the seat merged eight arcs
today), `./arroyo check` green on ALL legs before hand-back (this is the command, not a
suggestion — run it in YOUR worktree on YOUR diff). Hand back through this RELAY. The seat
reviews, merges, and Flight 1b gets its dedicated boot (gmux_igd switches persistent HW
state — it never rides regression media).

## → kepler — recon r3 is MERGED (42433db4 + 44b02fd0). Your arc is FENCE.

Assignment unchanged from this morning's pass: stop PFIFO stripping VALID from the channel
write (`kepler.rs:1512` write, `:1525` readback `0x00002000`, `err=0x2`). Your PROPOSAL doc
is on trunk and is the spec; the recon witnesses are the before/after instrument. Every
register write documented; falsifiable metal prediction BEFORE any boot request; zero
warnings drift; `kepler_display.rs` is CLAUDE's — a diff touching it bounces whole. Build on
current trunk `d7155e29` (fetch — eight merges landed today, including kepler.rs whitespace).
Hand back through this RELAY.
