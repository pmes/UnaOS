# Aulë

**Crate:** `handlers/aule` · **Edition:** Rust 2024 · **Status: Partial (early scaffolding)**

Aulë is the build handler for UnaOS userspace. Its responsibility is to compile
the current workspace by selecting and driving the correct toolchain for the
detected project type, and to stream the resulting build output back to the rest
of the system.

## What it does today

The implemented surface is small and deliberately concrete:

- **`Aule::new(path)`** — constructs a handler bound to a workspace directory. It
  uses `elessar::Context` to classify the directory into a `Spline` (the Elessar
  term for a detected project type: `UnaOS`, `Rust`, `Web`, `Python`, or
  `Void`).
- **`Aule::forge()`** — the core entry point. It maps the detected `Spline` to a
  build command and spawns it as a background subprocess:
  - `UnaOS` / `Rust` → `cargo build`
  - `Web` → `npm run build`
  - `Python` → `python setup.py build`
  - `Void` → no-op
  `forge()` returns immediately. Two reader threads consume the child's stdout
  and stderr line by line and currently print them to the process console.
- **`create_view(tx)`** — builds a GTK4 widget (an "Ignite" button) for the
  Linux GUI. Clicking it sends `gneiss_pal::Event::AuleIgnite` over the supplied
  channel, which a vessel translates into a build request.

## How it plugs into the bus

Aulë implements `bandy::BandyMember`, the trait for a component that publishes on
the message bus:

```rust
impl BandyMember for Aule {
    fn publish(&self, topic: &str, msg: SMessage) -> Result<()> { /* ... */ }
}
```

In the intended design, `forge()` streams each line of build output as an
`SMessage` on the `Synapse` (the `bandy` broadcast bus), so that a subscribed
GUI can render diagnostics and progress live — the same publish/subscribe pattern
every UnaOS handler uses (handlers never call each other directly; they exchange
`SMessage` values on the Synapse).

## Status

**Partial.** The toolchain-selection and subprocess-spawning core works, but the
bus wiring is not yet connected:

- `forge()` writes build output to stdout/stderr via `println!` rather than
  emitting `SMessage`s. No `SMessage` variants are produced or consumed yet.
- `BandyMember::publish` is a stub that logs its arguments instead of firing on a
  `Synapse`.
- There is no async `ignite(...)` entry point and no `Synapse` subscription loop;
  the handler is driven by direct method calls, not by subscribed bus messages.
- Diagnostics are raw text lines; structured parsing (e.g. Cargo's
  `--message-format=json`) is not implemented.

Requires a host Rust toolchain (and Node/Python for those project types). The
broader workspace snapshot-and-packaging pipeline that Aulë is expected to own
(see `docs/dev/USERLAND/ARCHITECTURE.md` §4) is design-stage only.

## See also

- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md) — userspace component model (libraries / handlers / vessels) and the Bandy bus.
- [`docs/CODEX.md`](../../docs/CODEX.md) — system canon and full handler manifest.
