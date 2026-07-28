# Principia

**System configuration and policy handler for UnaOS.** Principia is the
domain service responsible for persistent system settings — owning where they
live, validating changes, and broadcasting the results to the rest of userspace.

## Preferences — the settings surface

Principia owns the **preference store**: the namespaced, typed, persistent
settings the rest of userspace reads. This is the mechanism behind the OS
Settings surface; it is live today and serves over the bus.

### Addressing and types

A preference is addressed by a **namespace** — a per-app/domain string
(`aether`, `stria`, `system`) — and a **dotted key** within it (`homepage`,
`window.width`). A value is one of four scalar types (`bandy::PrefValue`):
`Str`, `Int`, `Float`, `Bool`. That domain is exactly what a TOML scalar carries
losslessly, so nothing is retyped by a save/load cycle (a whole-numbered float
stays a float).

**Defaults live with the consumer, never in the store.** A `get` on an unset key
answers `None`; the consumer applies its own default. The store therefore never
has to know what any app considers reasonable, and a settings file only ever
contains choices a user actually made.

### File format

`~/.config/unaos/preferences.toml` (resolved via `dirs::config_dir()`): one TOML
table per namespace, dotted keys expanded into sub-tables. It is meant to be
readable and hand-editable.

```toml
[aether]
homepage = "https://una.os/"

[aether.window]
height = 800
width = 1280
```

Because a key expands into TOML tables, a key cannot be both a value and a
table: `window` and `window.width` collide. The collision is rejected at set
time (with a `PrefError`), so the in-memory cache never holds something that
cannot be persisted. Namespaces and key segments are non-empty `[A-Za-z0-9_-]`
identifiers.

**Every write is atomic**: a set serializes the whole document to a sibling temp
file in the same directory, flushes and `fsync`s it, then `rename`s it over the
real file. A concurrent reader — or a crash — sees the old file or the new one,
never a partial one. If the write fails, the in-memory cache is rolled back, so
cache and file never disagree. A malformed file on load is an error rather than
a silent empty start (which would let the next set overwrite a user's settings
with nothing); the handler then serves an empty store from a quarantine name and
leaves the original untouched.

### Bus verbs

All carried on `SMessage::Principia(PrincipiaCommand)`:

| Direction | Message | Behaviour |
| --- | --- | --- |
| In | `PrefGet { ns, key }` | Read one preference. |
| Out | `PrefValueIs { ns, key, value: Option<PrefValue> }` | The answer; `None` = unset. |
| In | `PrefSet { ns, key, value }` | Validate, persist atomically. |
| Out | `PrefChanged { ns, key, value }` | Broadcast after every successful set — both the acknowledgement and the live-update signal running apps subscribe to. |
| Out | `PrefError { ns, key, message }` | A rejected set (bad namespace/key, path collision, failed persist). |
| In | `PrefList { ns }` | Every key set in one namespace. |
| Out | `PrefListIs { ns, entries: Vec<(String, PrefValue)> }` | The answer, sorted by key; an unknown namespace lists empty. |

### Running it

`principia::ignite(synapse)` subscribes and serves the loop. When a caller needs
to fire commands immediately after spawning, `principia::serve(synapse, rx,
handler)` takes a receiver the caller subscribed *before* the spawn, so nothing
in between is missed. `Principia::with_config_dir(dir)` opens the handler against
an explicit config lobe (tests, and any future multi-profile boot).

### Queued

- **Live update on external change.** Principia is the writer of record; an edit
  made to `preferences.toml` underneath a running Principia is not noticed until
  the next load. A file watcher that reloads and emits `PrefChanged` per delta
  is the follow-up.
- **The GUI surface.** Settings are served but have no face yet; a quartzite
  view over `PrefList`/`PrefSet` is the natural next step, and the schema needed
  to render a *good* one (labels, ranges, enums per key) is not defined.
- **First consumers.** Aether's homepage (`aether`.`homepage`) and window size
  (`aether`.`window.width` / `window.height`) are the intended first two;
  wiring them belongs to the aether-shell lane, not here.
- **Policy levels for helm.** The charter's law layer (below) rides this same
  store once the first drivable domain lands — the store is the mechanism, the
  levels themselves are not yet defined.

## The system root

The original capability, unchanged: managing the **UnaOS system root** (the
workspace path the rest of the system operates against).

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

Principia's vocabulary is the `PrincipiaCommand` sub-enum
(`libs/bandy/src/signals.rs`): the preference verbs tabled above, plus the
system-root pair:

| Direction | Message | Behaviour |
| --- | --- | --- |
| In  | `PrincipiaCommand::SetSystemRoot(PathBuf)` | Validate the path; if valid, persist it and emit the change below. |
| Out | `PrincipiaCommand::SystemRootChanged(PathBuf)` | Confirms the new root to any subscriber (e.g. the GUI, Matrix). |

`process_impulse` takes one inbound message and returns at most one outbound
message, which the caller publishes; `serve`/`ignite` are the loop that does
that against a live Synapse. Replies and broadcasts are inert as input —
Principia hears its own output on the broadcast bus and must not answer it.

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

**Live for preferences and the system root; design-stage beyond them.**

- The preference store and the system-root logic are implemented, tested
  (`cargo test -p principia`) and served over the Synapse. The crate is a
  workspace member and exposes the conventional entry point (`ignite`/`serve`);
  no vessel binds it yet.
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
