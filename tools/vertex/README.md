# Vertex

A small command-line tool that reports a node's identity and status by sending a
single JSON presence packet over UDP.

## Overview

Vertex is a one-shot CLI: it takes an identifier and a status, normalizes the
status to a canonical form, serializes the pair to JSON, and fires it as a single
UDP datagram to a target host. It does not run a daemon, hold a connection, or
wait for a reply — invoke it, it sends one packet, and exits.

It exists to let an external node announce *who it is* and *how it is doing* to a
UnaOS host with the minimum possible footprint (no GUI, no message bus, no
handler runtime).

## What it does

1. Parses three arguments: the identifier, the status, and an optional target
   address (default `127.0.0.1`).
2. Maps the supplied status string to a canonical status name (see below).
3. Serializes `{ id, status }` to JSON via `serde_json`.
4. Binds an ephemeral UDP socket (`0.0.0.0:0`) and sends the JSON datagram to
   `<target>:4200`.
5. Prints a confirmation (target and payload) on success; exits non-zero on a
   serialization, bind, or send error.

### Status mapping

`map_status` accepts either a canonical name or a color alias and returns the
canonical name. The recognized pairs are:

| Input (case-insensitive) | Canonical |
| --- | --- |
| `online` / `green`    | `Online`   |
| `oncall` / `teal`     | `OnCall`   |
| `active` / `seafoam`  | `Active`   |
| `thinking` / `purple` | `Thinking` |
| `paused` / `yellow`   | `Paused`   |
| `error` / `red`       | `Error`    |
| `offline` / `grey`    | `Offline`  |

Any unrecognized value is passed through unchanged, so a status the CLI does not
yet know about can still reach a backend that does.

## Usage

```
vertex <ID> <STATUS> [--target <IP>]
```

```
vertex s9-mule green                  # → {"id":"s9-mule","status":"Online"} to 127.0.0.1:4200
vertex s9-mule Thinking --target 10.0.0.5
```

The destination port is fixed at `4200`.

## Public surface

This is a binary crate (`main.rs`); its items are internal rather than a library
API. The notable definitions are:

- `Cli` — the `clap`-derived argument parser (`id`, `status`, `--target`).
- `Packet` — the `Serialize` payload struct (`id`, `status`).
- `map_status(&str) -> String` — the status/color normalization function. Covered
  by a unit test (`tests::test_map_status`).
- `main()` — argument parsing, serialization, and the UDP send.

## How it fits into UnaOS

Vertex is one of the command-line vessels under `tools/` described in the
[userspace architecture](../../../docs/dev/USERLAND/ARCHITECTURE.md). Unlike a
GUI vessel such as Lumen, it does not compose handlers or run on the Bandy
message bus (`SMessage` / `Synapse`); it is a self-contained utility that emits a
status signal over the wire to a listening UnaOS host.

## Status

Implemented. Single-file CLI with the argument parser, status mapping (unit
tested), and the UDP send path in place. The receiving endpoint and the wire
schema beyond `{ id, status }` are outside this crate.
