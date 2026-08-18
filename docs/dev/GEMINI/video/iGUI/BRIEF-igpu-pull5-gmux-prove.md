STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-igpu-pull5.md`, this directory)

# BRIEF — iGPU pull 5: prove the gmux protocol

Coordinator-authored (2026-07-22, post sitting #9 boot 1).

## Where it stands (KEPLER-METAL-LOG.md #9)

The handshake got us distinct, stable per-register bytes (SW_DISP=0x03,
SW_DDC=0x02, POWER=0x03 at both points) — the mux is answering — but the
version self-test failed, so the decode stays quarantined. If 0x03 were
trustworthy it reads "discrete owns the panel", which would relocate the
whole display question to the Kepler side. Proving the protocol is the
gating move for the entire display strategy.

## What pull 5 must cover (read-only, all raw + gated as before)

1. **gmux variant facts** — the known hardware variants (classic vs indexed;
   version register locations and widths per variant; any revision where the
   version lives at different indexes or is read as a multi-byte value).
   Known fact to check first: on some gmux revisions the version is read as
   a 32-bit value from a single port/index rather than three byte registers
   — derive the variants table with citations (hardware facts only).
2. **Alternate self-tests** — if no version variant proves out, propose a
   different known-shape register as the protocol proof (e.g. max-brightness
   or a register with a constrained legal range), so the gate has a second
   chance to pass honestly.
3. **Both-points read** as before; decode stays gated on a PASSED self-test.
4. If the protocol proves and SW_DISPLAY really is 0x03/discrete: state the
   implication in the report (display work pivots to Kepler-side scanout
   decode; the iGPU all-dead canon becomes EXPECTED behavior, not a paradox)
  — but no scope change in this pull.

ABI/land-review laws unchanged. Metal owed: sitting #10.
