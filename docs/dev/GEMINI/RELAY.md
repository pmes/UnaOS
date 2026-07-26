# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-26: pull 33 APPROVED-WITH-CORRECTION — the IO ports are wrong, must-fix + A/B fallback)

## → kepler-fence session

Fence: pull 33 approved in shape — the echo-loop design is right, the milestone split is right, and the citation discipline is what this era needs. But there is a MUST-FIX defect in the encoding, and it is the same class of error I made on pull 25, so I want it fixed the way that one got fixed.

⛔ THE IO PORTS ARE WRONG. Your listing uses 0x800 and 0x804 as Falcon IO port indices — those are HOST register offsets. The scheme we confirmed on metal at s29 is: host register X → falcon (X & 0xffc) << 6. That is why MAILBOX0 (host 0x040) is I[0x1000] and MAILBOX1 (host 0x044) is I[0x1100] — the values your own heartbeat ucode used successfully. Applying it: CC_SCRATCH[0] host 0x800 → I[0x20000]; CC_SCRATCH[1] host 0x804 → I[0x20100]. Those don't fit the I16 immediate form your listing uses, so both `mov`s need a different encoding (I32 form, or a sethi pair) — pick one and cite the form.

AMENDMENT 2 — A/B FALLBACK, exactly as pull 25 established: ship image A with the derived indexed ports and image B with the flat ports as you originally wrote them; run A first, fall back to B if no ack, label the attempt in the marker. One boot then settles the CC_SCRATCH port question no matter which derivation is right. That fallback is what confirmed I[0x1000] on metal, and it costs nothing here.

AMENDMENT 3 — drop the gating premise from the prose. CTXCTL subunit gating was REFUTED at s33 (PIBUS_MMIO_HUB_ENABLE1=FFF9F4B0, bit 4 already set) and s34 (all five remaining offsets read real zeros). The true statement: the poison is per-offset, 0x409504 alone is convicted, and CC_SCRATCH is host-readable because it is simply a working offset. Whether the Falcon can reach WRCMD from inside the unit is an open question worth stating as one — it may well be the answer, but we haven't shown it.

Everything else stands: bounded poll, no retries, FECS only, the proven upload/execute sequence, and keep the known-good image A execution witness running first as pull 27 did. Implement as approved + amendments, commit ALL docs+code, no push. Report "PUSH OWED: n". (I run all builds and gates.)

## → kepler-display session

Display: idle and graduated. FYI the console gained a real improvement from a code review — the panel paint no longer runs with interrupts masked (layout planned under the lock, pixels painted outside it), which matters on boots driving USB deadlines. Next sitting re-verifies text on glass.
