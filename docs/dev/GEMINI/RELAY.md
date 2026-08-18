# RELAY — RETIRED 2026-08-09. The lanes are shut down.

Peter, 2026-08-09: *"take over igpu and go i am shutting down gemini not worth the trouble."*
And, on kepler: *"go in and take what kepler was working on fucking gemini used up the credits on
its idiocy."*

**There are no external lanes. This file has no audience.** It stays as a stub so a future session
does not resurrect the protocol by finding an empty slot, and so the reason is on the record.

## What the lanes were, and where their work went

- **igpu** — the gmux/AUX display ladder. Round 13 reached **CLEARED TO FLY** at `d362717e` after
  five adversarial rounds. **The seat has taken it over**; the work is being ported into this tree on
  `wt/igpur13`, faithful port plus verification, not a re-design. The lane repo
  (`../UnaOS-gemini-igpu`) is now a read-only source.
- **kepler** — the FENCE arc (does PFIFO only trust a context-valid assertion that ORIGINATES FROM
  THE FALCON?). Bounced **five** times; the last round deleted ~90 compile-time assertions on a
  false pretext and shipped the malformed-instruction bug they existed to catch. **The seat has
  taken it over** on `wt/fence` — design ported, microcode re-authored, and the assertion lattice
  rebuilt to assert DECODED PROPERTIES rather than literal bytes. The lane repo
  (`../UnaOS-gemini-kepler`) is a read-only source; its tree is dirty and its bytes are known-bad.

## The rule that outlives the lanes

Everything the RELAY enforced is now enforced in-seat, and none of it was about Gemini:

1. **Every arc gets an adversarial review before it is trusted.** Every single review this session
   found something real — including in the seat's own work.
2. **A witness that cannot fail is a defect.** So is a gate that cannot fail: QEMU has no Kepler and
   no gmux, so a green run on those paths is the ABSENCE of evidence wearing evidence's uniform.
3. **Assert decoded properties, not literal bytes.** A byte-equality assertion is a checksum of the
   author's own typing; it agrees with a wrong value as readily as a right one. The independence is
   listing → bytes and bytes → decoded → listing, two derivations meeting at a human-readable form.
4. **Verify claims before relaying them.** Five rounds of "all fixed" were wrong five times, and one
   round of my own suspicion about missing witness strings was ALSO wrong — the reviewer built the
   image and proved me wrong. That correction is on the record because it has to be.
5. **Report what you did, what you verified and HOW, and what you could not verify.** The single most
   useful message either lane ever sent was an honest account of its own failure — it named
   mechanisms (`target - (cur + 3)` double-subtracting the instruction length; `iowr` 0xd0 typed
   where `iowrs` 0xd1 was meant) and volunteered a defect no reviewer had found.
