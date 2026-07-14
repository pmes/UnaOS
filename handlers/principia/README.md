# Principia

**System configuration and policy handler for UnaOS.** Principia is the
domain service responsible for persistent system settings — owning where they
live, validating changes, and broadcasting the results to the rest of userspace.

## What it does today

The current implementation is a single capability: managing the **UnaOS system
root** (the workspace path the rest of the system operates against).

`src/lib.rs` exposes a `Principia` struct:

- `Principia::new()` — loads the persisted system root from
  `~/.config/unaos/principia.toml` (resolved via `dirs::config_dir()`),
  creating the config directory if needed.
- `process_impulse(&mut self, &SMessage) -> Option<SMessage>` — the inbound
  message handler. It reacts to a single command, validates it, persists the
  change, and returns an outbound message to be published on the bus.
- `validate_root(&Path) -> bool` — accepts a path only if it is an existing
  directory containing a `crates/` or `libs/` subdirectory (i.e. a plausible
  UnaOS source tree).

When a valid root is set, Principia writes it to `principia.toml` so the choice
survives a restart.

## Bus integration

UnaOS handlers communicate over **Bandy**, the userspace message bus
(`libs/bandy`): a single `SMessage` enum carried on the **Synapse**, a
multi-producer/multi-consumer broadcast channel. Handlers do not call each other
directly — they publish and subscribe to `SMessage`.

Principia uses two variants of `SMessage::Principia(PrincipiaCommand)`
(`libs/bandy/src/signals.rs`):

| Direction | Message | Behaviour |
| --- | --- | --- |
| In  | `PrincipiaCommand::SetSystemRoot(PathBuf)` | Validate the path; if valid, persist it and emit the change below. |
| Out | `PrincipiaCommand::SystemRootChanged(PathBuf)` | Confirms the new root to any subscriber (e.g. the GUI, Matrix). |

`process_impulse` returns the `SystemRootChanged` message on success and `None`
otherwise; the caller is responsible for publishing the returned message on the
Synapse.

## Safety levels — the law an AI runs under (planned)

Beyond the system-root capability, Principia is chartered to own the **safety
levels** for anything an AI will drive. This is the *law* layer of the UnaOS
safety stack, **law → authority → interlock**
([`docs/dev/USERLAND/RECONCILIATION-2026-07.md`](../../docs/dev/USERLAND/RECONCILIATION-2026-07.md)):

- **Principia states the law.** For each **action-domain**, the user chooses a
  level along a spectrum — from **never** (the AI may not act), through **ask**
  (the AI must get human confirmation before each action), to **autonomous
  within bounds** (the AI may act on its own inside stated limits). Principia is
  the single, auditable place these choices are declared and persisted. It does
  not itself carry out or block any action — it publishes the policy.
- **Helm holds the authority.** The `helm` handler ([`handlers/helm`](../helm))
  reads Principia's published levels and *enforces* them: every AI-initiated
  physical action passes through helm, which resolves it against the relevant
  level to **pass / ask / refuse**.
- **The kernel helm core is the interlock.** Beneath both, the Ring 0 core at
  `unaos/libs/sys/helm` is the hard interlock that does not negotiate.

Principia is the law everyone reads; helm is the one place authority over
consequences lives. Actuators (machines an AI drives through helm) are the first
action-domain; other consequence domains are added as they arrive. The concrete
`SMessage` variants that publish and update these levels are defined when the
first drivable domain lands.

## Status

**Partial — design-stage beyond the system-root capability.**

- The system-root logic above is implemented and self-contained.
- This crate currently has **no `Cargo.toml`** and is **not a workspace member**,
  so it does not build as part of `cargo build`. It also does not yet expose the
  conventional async entry point (`ignite(synapse: Synapse, …)`) or a subscribe
  loop that drives `process_impulse` from live Synapse traffic; integrating it
  into a vessel is outstanding work.
- The broader vision for Principia — a schema-driven settings UI generated from
  per-handler configuration schemas, versioned config via the Vairë handler,
  semantic validation of risky settings via the Vein handler, and host
  (Linux/macOS/Windows) dotfile management — is **not implemented**. It is
  recorded as the intended direction in `docs/CODEX.md`, where Principia is the
  System "Policy Engine."

## See also

- `docs/dev/USERLAND/ARCHITECTURE.md` — userspace component model (libs /
  handlers / vessels) and the Bandy/Synapse bus.
- `docs/CODEX.md` — handler manifest and the long-term role of Principia.
- `libs/bandy/src/signals.rs` — `SMessage` and `PrincipiaCommand` definitions.
- [`handlers/helm`](../helm) — the control-authority handler that enforces the
  safety levels Principia states.
- [`docs/dev/USERLAND/RECONCILIATION-2026-07.md`](../../docs/dev/USERLAND/RECONCILIATION-2026-07.md)
  — the law → authority → interlock safety model.
