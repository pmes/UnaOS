# V3D-2 — landing report (hw-pi4)

**Arc:** PI-V3D-2 (Peter-approved fix-forward for PI-V3D-1's metal false-pass). Branch `hw-pi4`.
**Ground truth being fixed:** PI-V3D-1's probe FALSE-PASSED on metal — it accepted `0xdeadbeef`
open-bus poison as a live IDENT. Two legs shipped; metal verification deferred to the next attended
Pi sitting (QEMU raspi4b does not model V3D — this is a QEMU/metal divergence class by construction).

## What landed

**Leg 1 — poison-honest probe** (`arch/aarch64/v3d.rs`):
- New `is_poison()` rejecting `0xffffffff` AND `0xdeadbeef` (mirrors `pcie_probe::is_poison`).
- New `V3dPresence { Up(u32), Down, Poison(u32) }` + `probe_hub_ident0()` replacing the
  zero-only `ident_looks_live()`. Three discriminated verdicts, each a distinct serial line;
  only `BLOCK-UP` proceeds to core-register access. `BLOCK-DOWN` and `BUS-POISON` both return
  BEFORE any core read, so neither can raise the forbidden `AARCH64 EXCEPTION`.
- Bounded settle-retry window (finite off CNTPCT) so a freshly powered block gets a moment to
  answer before a poison read is declared `BUS-POISON`; a `0` read is an immediate `BLOCK-DOWN`.

**Leg 2 — enable-sequence gap** (`arch/aarch64/mailbox.rs` + `v3d.rs`):
- Root cause: `SET_CLOCK_RATE` programs the frequency but does NOT open the clock GATE (RPi
  firmware treats rate and enable-state independently) — power+rate ACKed while the block stayed
  powered-but-unclocked, reading open-bus poison.
- New `mailbox::set_clock_state` (tag `SET_CLOCK_STATE 0x00038001`) opens the gate explicitly and
  requires the firmware to confirm the clock present AND active. Sequence is now power domain →
  set rate → **enable gate** → bounded settle → probe.

All changes are inside `#[cfg(feature = "v3d")]`-gated code — knob-off `kernel8.img` byte-identity
to baseline is preserved by construction.

## The three-verdict probe message texts (verbatim)

- BLOCK-UP:
  `:: V3D: probe verdict BLOCK-UP — hub IDENT0 = {:#010x} (live V3D identity) ::`
- BLOCK-DOWN:
  `:: V3D: probe verdict BLOCK-DOWN — hub IDENT0 = 0x00000000 (block absent/unpowered; expected in QEMU raspi4b) — GPU bring-up skipped, graceful degradation ::`
- BUS-POISON:
  `:: V3D: probe verdict BUS-POISON — hub IDENT0 = {:#010x} (open-bus/firmware fill, NOT a live register — the powered+clocked path did not bring the block up) — GPU bring-up skipped, fail-closed ::`

New enable line: `:: V3D: clock id 5 gate ENABLED (active) ::`

## Gate results (verbatim)

- `./arroyo check`: `✅ aarch64 OK` (x86 + aarch64 type-check green; pre-existing warnings only).
- `UNAOS_V3D=1 ./arroyo kernel8-test` → `mbench.py --spec pi4-regression.spec --replay`:
  `✅ MBENCH PASS — 46/46 required witnesses, 0 forbidden hit(s), 194 lines scanned`.
  0 `AARCH64 EXCEPTION`, 0 `PANIC`, 0 `-> FAIL`. V3D chain in QEMU: power domain 10 ON → rate
  set 500 MHz → gate ENABLED (active) → `probe verdict BLOCK-DOWN` (QEMU's 0 read — correct,
  no core access, no fault).
- `./arroyo test-arm 22`: `✅ aarch64 test complete`; 0 exceptions/panics/FAIL.

Note: the pi baremetal kernel does not print an "MBENCH" summary line itself — the formal verdict
is the `mbench.py` replay against `pi4-regression.spec` (per the hazards ledger). The
`kernel8-test` capture window ends mid-battery at the BANDY witnesses; the replay verdict (46/46)
is the gate of record.

## Docs updated

- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` §PI-V3D: M1 bullet (added `SET_CLOCK_STATE` step), the
  QEMU-serial example block, and a new `### PI-V3D-2` subsection documenting both legs + the
  false-pass root cause.
- `~/.claude/plans/unaos/metal/unaos-metal-pi4.md`: new `▶ PI-V3D-2 LANDED — metal leg PENDING`
  entry (no media staged, per brief; the LC-pi stages fresh at the next bench prep); the stale
  V3D-1 staging record marked SUPERSEDED.

## Flagged

- Nothing out-of-lane; no protection weakened; all edits in the Pi/aarch64 V3D + video lane.
- The QEMU run confirms QEMU raspi4b models `SET_CLOCK_STATE` (reports gate active); on metal the
  discriminating expectation is `BLOCK-UP` with a live identity. If the block still reads poison on
  silicon, the probe now fail-closes with the `BUS-POISON` line instead of false-passing — that is
  the designed honest outcome and feeds the next enable-sequence refinement (not a regression).
