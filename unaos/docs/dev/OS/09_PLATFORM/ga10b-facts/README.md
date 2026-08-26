# GA10B facts-import area

This directory holds **reviewed hardware-fact files** extracted from restricted vendor sources
under the quarantined clean-room terms of
[`CLEAN_ROOM_POLICY.md`](../../../../../../docs/MANIFESTO/CLEAN_ROOM_POLICY.md) §6. The pipeline
that produces these files is described in [`../ga10b-clean-room.md`](../ga10b-clean-room.md).

Rules for anything in this directory:

- **Facts only.** Register offsets, bit-field layouts, magic constants, and required ordering,
  each carrying an `nvgpu:file:line` provenance **pointer**. Pointers locate the fact in the
  source of record; they never reproduce it. No code, no copied prose, no expression.
- **Reviewed before import.** A file lands here only after the §6 terms review. The review is a
  conflict-of-interest guard: the extractor does not clear its own import alone — an independent
  seat re-checks against the terms and that ack is recorded in the import commit (or noted at the
  top of the file until the ack lands).
- **Group boundary.** The contributor who extracted a file is Group A for that feature and may
  not author the UnaOS implementation of it (policy §2). These files are the Group A → Group B
  handoff; a Group B implementer works from the facts here, never from the quarantine source.

Files:

- [`ga10b-probe-rung1.facts.md`](ga10b-probe-rung1.facts.md) — facts for the first read-only
  probe rung (BPMP power query; falcon/priscv boot-ROM handshake; security-state fuses; die
  census). Source: L4T r36.4.3 `nvgpu`.
