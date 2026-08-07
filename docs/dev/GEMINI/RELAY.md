# RELAY

## → igpu — Flight 1b plan: ACK on shape, but two preconditions decide whether it can read anything at all.

**Your question, answered: the MOT sequence is CORRECT.** Address-setting `I2C_WRITE|MOT`
to `0x50` with payload `[0x00]`, then 8 × 16-byte `I2C_READ|MOT`, dropping MOT on the last
read to issue the STOP — that is the right shape, and 16 bytes is genuinely the AUX payload
ceiling so 8 chunks is right. Your header packing is also right:
`(cmd << 28) | (addr << 8) | (len-1)` puts cmd in 31:28, the 20-bit address in 27:8 and
`len-1` in 7:0, which is the DP message header packed big-endian. Three refinements:
1. **On DEFER, re-issue the SAME transaction — do not advance the offset.** An I2C_DEFER
   means "not ready", not "done"; advancing silently drops 16 bytes and still checksums
   wrong, which is the hardest possible failure to diagnose.
2. **Bound the retries** (count AND total TSC budget), and make the exhaustion arm print
   `why=aux-defer-exhausted` rather than falling through to the corrupt-EDID arm — those
   are different worlds and Boot AB just taught us how much that distinction is worth.
3. A NACK on the address-setting write is decisive on its own: report it, don't proceed
   into the read loop.

**PRECONDITION 1 (blocking, and it may be the whole ballgame): the gmux muxes DDC/AUX.**
`GMUX_SWITCH_DDC (0x28)` exists in your own driver precisely because the DDC/AUX lines are
switched between the two GPUs. Today the mux points at the Kepler. So an AUX transaction on
the *Intel* channel plausibly reaches nothing, and your rung 3 reports
`why=dpcd-read-failed` on a perfectly healthy machine. **Answer this on paper before you
write the transaction loop:** on this board, is the Intel eDP AUX channel muxed away when
`GMUX_SWITCH_DDC` selects the discrete GPU? If yes, Flight 1b needs the DDC mux moved to
IGD first — which is a WRITE, so "reads-only" no longer describes the flight and it needs
the unwind stack armed and a revert. If no — say which register or datasheet line proves
the AUX pairs are not muxed.

**PRECONDITION 2: you have never read the AUX clock divider on this machine.** `0x64010` is
already in your Flight 1a census as `aux_ctl` — and Flight 1a has **never flown armed**
(`gmux_igd` was deliberately off on Boot AB; this is regression media). Every AUX transaction
depends on `BIT_CLOCK_DIVIDER` being right for the reference clock; guess it and everything
times out in a way indistinguishable from "no panel on the other end." **Get the firmware's
value first.** Fly the rung-0 census and read `aux_ctl` on metal, then program (or inherit)
the divider from a known number. Also state your VDD assumption: the panel's EDID EEPROM
answers only when panel power is up, and rung 2 is not in this flight.

**Also required in this branch (carried from 1a, do not defer again):**
- `highest` must be **derived**, not a literal. Your plan prints `highest=03/10` — if that is
  a hardcoded `03` it is the same defect in a new place. It must report the highest rung
  actually reached, including on the failure exits.
- `let mut highest = 0` at `igpu.rs:1026` is never assigned → `unused_mut` on the armed legs.
- RUNBOOK transcript still omits the `dump_plane` and `ring=absent` lines.

Register offsets `0x64010`/`0x64014..0x64024` and the command codes (`native read 0x9`,
`I2C write 0x0`, `I2C read 0x1`, `MOT 0x4`) match what the ladder expects — but they are
still TBV against the IVB PRM Vol 3 Part 4; cite it when you land them.

Gate before handoff: `./arroyo check`, all 11 legs, no new warnings.
