# helm — control-authority handler ("The Wheel")

helm is the UnaOS handler that holds **control authority** over every physical
action an AI initiates. Nothing an AI drives — an actuator, a machine, any
consequential physical output — reaches the hardware without passing through
helm's gate. It is the newest entry in the handler manifest
([`docs/CODEX.md`](../../docs/CODEX.md), Amendment I, 2026-07-14): **The Wheel**,
the handler that did not exist because nothing did this job before.

The image is a ship's helm: the wheel and the captain's voice at one station.
Direct human control and commanded intent meet in one place, and one authority
decides which is in effect at any moment.

**Status:** design-stage (not yet implemented). This document describes the
intended design. The crate currently contains no working code; the entry point
and message contract below are the planned interface, not a shipped one.

## Responsibility

Every AI-initiated physical action passes through helm. For each such action
helm:

1. **Reads the law.** It consults `principia`'s safety levels — the user-chosen
   policy, per action-domain, ranging from "never" through "ask" to "autonomous
   within bounds" (see `handlers/principia`).
2. **Decides.** It resolves the action against that level to one of three
   outcomes:
   - **pass** — the level authorizes the action; helm forwards it.
   - **ask** — the level requires a human in the loop; helm surfaces the request
     for confirmation and forwards only on approval.
   - **refuse** — the level forbids the action; helm rejects it and reports why.
3. **Records.** The decision and its basis are made legible, so authority over
   consequences lives in one auditable place rather than being scattered across
   handlers.

helm is where authority over consequences lives — the single place a user looks
to understand and govern what the system is permitted to do on its own.

## The safety stack: law → authority → interlock

helm is the middle of three layers defined in the July 2026 reconciliation
([`docs/dev/USERLAND/RECONCILIATION-2026-07.md`](../../docs/dev/USERLAND/RECONCILIATION-2026-07.md)):

- **principia — the law.** *States* the safety levels. It does not enforce them;
  it declares, per action-domain, what the user has permitted.
- **helm (this handler) — the authority.** *Holds* control authority in
  userspace: reads principia's levels and decides pass / ask / refuse for each
  AI-initiated action.
- **the kernel helm core — the interlock.** The hard layer beneath that does not
  negotiate.

### Distinct from the kernel helm core

This handler is **not** the kernel `helm` core at
[`unaos/libs/sys/helm/`](../../unaos/libs/sys/helm) — keep them clearly
separate:

| | `handlers/helm` (this crate) | `unaos/libs/sys/helm` (the core) |
| --- | --- | --- |
| Ring | Ring 3 (userspace handler) | Ring 0-embeddable (`no_std`, `forbid(unsafe_code)`) |
| Job | Reads policy, decides pass / ask / refuse | The hard interlock: DISARM / MANUAL / AUTO + FAILSAFE latch |
| Nature | Negotiates against user-chosen law | Does **not** negotiate — forces neutral, latches, human estop wins |
| Scope | All AI-initiated physical actions | Per-machine safety domains (`src/rover/` first — failsafes do not generalize) |

The handler decides *whether* an action is permitted; the core guarantees the
machine parks safely regardless — even through a kernel fault or panic. The
handler reads the law and negotiates; the core is the layer under everything
that just stops the machine. See the core's README for its DISARM/MANUAL/AUTO
charter and the transmitter-as-human-estop invariant.

## Integration with the message bus

Like every UnaOS handler, helm is a self-contained crate exposing an async entry
point (by convention `ignite(...)`) and communicates only over the Bandy
broadcast bus (the Synapse). It subscribes to AI-initiated action requests,
resolves them against principia's published levels, and emits pass / ask /
refuse decisions as `bandy::SMessage`. The concrete `SMessage` variants — the
action-request shape, the ask/approve round-trip, and the decision record — are
defined when the first drivable domain lands. That work is a deliberate,
reviewed change to the `bandy` enum.

## See also

- [`docs/CODEX.md`](../../docs/CODEX.md) — the handler manifest and Amendment I
  (Helm joins The 21).
- [`docs/dev/USERLAND/RECONCILIATION-2026-07.md`](../../docs/dev/USERLAND/RECONCILIATION-2026-07.md)
  — the law → authority → interlock safety model.
- [`handlers/principia`](../principia) — the policy engine that states the safety
  levels helm enforces.
- [`unaos/libs/sys/helm`](../../unaos/libs/sys/helm) — the kernel control-authority
  core (the hard interlock beneath this handler).
