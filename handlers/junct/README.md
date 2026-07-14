# junct — communications aggregation handler ("The Receiver")

junct is the UnaOS handler responsible for **receiving** human conversation. It
abstracts the fragmented landscape of messaging networks — Matrix, Email, IRC,
RSS — into a single inbound **Stream**, so the rest of the system sees one
conversation surface rather than a drawer full of apps.

**Status:** design-stage (not yet implemented). This document describes the
intended design. The crate currently contains no working code; the entry point
and message contract below are the planned interface, not a shipped one.

## Responsibility

Receive from every human conversation network and normalize it into one Stream.
junct is the counterpart to `vein`: the two are **symmetric platform
abstractions** — same shape, same shared chat view, a different party on the far
end.

- **junct** abstracts human conversation networks (Matrix, Email, IRC, RSS ->
  one Stream). Party on the far end: another person.
- **vein** abstracts AI providers (local / cloud -> one conversation). Party on
  the far end: an AI.

## Scope

- **Receive, don't render.** junct is a **headless** handler: it owns the
  capability of connecting to conversation networks and turning their traffic
  into normalized messages on the bus. It has **no chat UI of its own** — that
  is the job of the shared chat view (the same view serves both junct and vein).
- **One Stream, many networks.** Protocol adapters (Matrix, Email/IMAP, IRC,
  RSS) fold into a single unified inbound Stream with a common message shape,
  rather than one bespoke app per network.
- **Network access through the platform.** Connections are expected to go
  through `gneiss_pal` (its `net` module) rather than being re-implemented in
  the handler.

## Integration with the message bus

Like every UnaOS handler, junct is a self-contained crate exposing an async
entry point (by convention `ignite(...)`) and communicates only over the Bandy
broadcast bus (the Synapse). As a **producer**, it emits normalized inbound
conversation as `bandy::SMessage`; the shared chat view subscribes and presents
it. The concrete `SMessage` variants for the Stream are defined when the first
adapter lands.

## History

The crate previously contained an unrelated microphone-capture + live-FFT
spectrum path (a `cpal` audio input stream feeding `resonance`'s FFT and
publishing `SMessage::Spectrum`). That code was introduced by a
`google-labs-jules[bot]` session (commit `64bace4`, "feat(lumen): The
Awakening", 2026-02-19) and did not reflect junct's chartered communications
role — a comms handler has no business with microphone dependencies. It was
removed in the July 2026 userspace reconciliation (correction 2), returning
junct to a clean design-stage stub. See
[`docs/dev/USERLAND/RECONCILIATION-2026-07.md`](../../docs/dev/USERLAND/RECONCILIATION-2026-07.md).

Edition: Rust 2024. License: LGPL-3.0-or-later.
