# Comscan

The hardware I/O and signal handler for UnaOS: a bridge between the workspace
and external hardware over serial, GPIO, Bluetooth, and software-defined radio.

**Status: design-stage (not yet implemented).** This document describes the
intended design. There is no implementation crate yet — no `Cargo.toml`, no
`ignite(...)` entry point, no source. The behavior below is a specification, not
a description of working code.

## Responsibility

Comscan owns direct communication with hardware interfaces. Where most of the
system deals in files, workspaces, and rendered views, Comscan deals in raw byte
streams, link parameters, and wireless device discovery. It is the handler other
parts of the system use when they need to talk to a physical device — for
example, streaming a generated CNC/3D toolpath to a controller over USB serial.

## Scope (planned)

- **Serial / UART** — a terminal and byte pipe for microcontrollers and
  controllers (Arduino, ESP32, STM32, 3D-printer/CNC firmware), with baud-rate
  detection and a hex/ASCII view of raw traffic.
- **GPIO** — read/write of general-purpose I/O lines on supported hardware.
- **Bluetooth** — discovery and inspection of BLE devices, including raw
  advertisement data; pairing key material is delegated to the `holocron`
  secrets handler rather than stored by Comscan.
- **Software-defined radio (SDR)** — spectrum capture and demodulation for
  diagnostic and sub-GHz protocol work.

Comscan is intended to build on the serial/signal stack in `gneiss_pal`
(`src/net`) rather than re-implement host transport itself.

## Integration with the Synapse / SMessage bus

Like every UnaOS handler, Comscan is a self-contained crate that will expose an
async entry point (by convention `ignite(...)`), subscribe to the `Synapse`
broadcast bus, and react to `SMessage` variants. It does not call other handlers
directly. The planned message flow:

- **Inbound** — Comscan subscribes via `Synapse::subscribe()` and acts on
  commands addressed to it: open/close a port, set link parameters, write a byte
  stream to a device, start/stop a scan.
- **Outbound** — Comscan publishes via `Synapse::fire(msg)`: device-discovery
  results, received serial/wireless data, and link-status changes, for the GUI
  and other handlers to observe.

Dedicated `SMessage` variants for Comscan's commands and events are not yet
defined; adding them is a deliberate, reviewed change to the shared `bandy`
enum.

## Relationship to other handlers

- **Vug** (3D/CAD/CAM) generates toolpaths; Comscan streams them to the machine.
  This is the "design → make" path with no intermediate slicer or export step.
- **Holocron** holds pairing keys and other secrets; Comscan defers all key
  storage to it.

## See also

- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
  — the handler / Synapse / SMessage model.
- [`docs/CODEX.md`](../../docs/CODEX.md) — the full handler manifest.
