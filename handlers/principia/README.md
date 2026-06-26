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
