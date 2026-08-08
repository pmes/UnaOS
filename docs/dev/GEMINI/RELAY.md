# RELAY

## → kepler — recon r3 is MERGED. Your arc is now FENCE.

Recon r3 landed on trunk: merge `42433db4` + the conditioned whitespace strip `44b02fd0`
(seat applied it at merge — nothing owed from you on r3). `./arroyo check` green both arches.

**Assignment: FENCE — stop PFIFO stripping VALID from the channel write.**

The conviction from your own recon: the channel write at `kepler.rs:1512` goes out with
VALID set, the readback at `:1525` returns `0x00002000` (VALID gone), `err=0x2`. The FENCE
arc is the fix for that strip — your PROPOSAL (`docs/dev/GEMINI/video/Kepler/
PROPOSAL-kepler-fifo-valid-recon.md`, now on trunk) is the spec; the recon witnesses you
just landed are the before/after instrument. Requirements:

1. Every register write gets its justifying read documented, same as the recon discipline
   (the 504 law and zero-warnings-drift gates carry over).
2. A falsifiable metal prediction BEFORE any boot request: state what the `:1525` readback
   and `pfifo_err` will show when the fence works, and what a refute looks like.
3. **Lane boundary, unchanged:** `kepler_display.rs` is CLAUDE's — a diff touching it
   bounces whole. `video/wcx.rs` and `video/` are seat territory. You own `kepler.rs` +
   the Kepler docs.

Hand back through this RELAY when the diff is ready for review; the seat reviews, merges,
and stages the boot.
