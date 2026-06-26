# Geode

Archival and packaging handler for UnaOS: compression, signed archives, and a
lightweight container/runtime format for distributing software.

> **Status: design-stage (not yet implemented).** This document describes the
> intended design. There is no implementation in this crate yet — no
> `Cargo.toml`, no source, and no `ignite(...)` entry point. Nothing here
> reflects working code.

## Responsibility

Geode owns the system's archive and package format (`.geode`). It is the
counterpart to general-purpose tools such as Zip/7-Zip/tar and, for lightweight
isolation, to container runtimes such as Docker — covering both *storing* data
compactly and *packaging* applications for distribution.

## Planned capabilities

### Archival
- **Compression.** Modern, decompression-fast codecs (Zstd, LZ4) so archives
  open with low latency.
- **Indexed structure.** Archives carry a directory/metadata index rather than a
  flat byte stream, enabling random access and metadata queries without reading
  the whole file. The intent is to browse and stream entries (e.g. a media file)
  without a full extract.
- **Deduplication.** Identical blocks are stored once across files, primarily to
  reduce the size of backups and snapshots.

### Packaging / runtime
- **Capsules.** Package an application together with its dependencies into a
  single immutable `.geode` file for reproducible distribution.
- **Sandboxed execution.** Run packaged WebAssembly binaries in an isolated
  environment — lighter than full virtualization for most workloads.

### Integrity
- **Signing.** Each archive is cryptographically signed at creation, with
  signing/key material provided by the `holocron` handler (secrets/keyring), so
  origin and tamper status can be verified.
- **Manifest.** A human-readable list of contents and the permissions a
  contained application requires to run.

## Integration with the bus

Like all UnaOS handlers, Geode is intended to be a self-contained crate exposing
an async entry point (by convention `ignite(...)`) that attaches to the
**Synapse** — the `bandy` message bus over a Tokio broadcast channel. Handlers do
not call each other directly: Geode would `subscribe()` to receive `SMessage`
requests (archive/extract/inspect/sign operations) and `fire(...)` results back
onto the bus for the GUI and other handlers to consume. The specific `SMessage`
variants for Geode are not yet defined.

## See also
- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md) — userspace component model (vessels / handlers / libraries) and the Bandy bus.
- [`docs/CODEX.md`](../../docs/CODEX.md) — full handler manifest.
- `holocron` handler — signing and key material referenced above.
