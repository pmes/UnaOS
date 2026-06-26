# xenolith — virtualization handler (VMs / containers)

Xenolith is the UnaOS virtualization handler: it manages the lifecycle of
isolated guest environments — full virtual machines (KVM/QEMU) and system
containers — so that a vessel can run untrusted or experimental code without
touching the host. It is one of the userspace handlers described in
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md);
in the handler manifest ([`docs/CODEX.md`](../../docs/CODEX.md)) it is the
hypervisor frontend.

## Status

**Design-stage — not yet implemented.** This crate currently contains only this
README. There is no `Cargo.toml` and no `src/`; the API names, message types,
and behavior below describe the intended design, not working code. Treat every
snippet as a sketch subject to change once implementation begins.

## What it will do

Xenolith abstracts host virtualization backends (libvirt / QEMU for VMs,
cgroups + container runtimes for system containers) behind a Rust-native,
async API. The planned capability surface:

| Capability     | Description                                                            |
| -------------- | --------------------------------------------------------------------- |
| Spawn          | Create a VM from an installer ISO or a cloud image (QCOW2).            |
| Isolate        | Run untrusted code or system updates in a sandboxed guest.            |
| Snapshot       | Save and restore guest state on demand.                               |
| Network        | Manage guest networking (NAT / bridged) to the host or beyond.       |
| Pass-through   | Expose host USB/PCI devices to a guest for hardware testing.          |

The intended consumers are vessels that need a disposable target — for example
running a destructive shell script against a throwaway VM instead of the host,
or bringing up a different OS to check application compatibility.

## How it will plug into the bus

Like every UnaOS handler, Xenolith is expected to be a self-contained crate that
communicates only over the Bandy message bus — it publishes and subscribes to
`SMessage` on the `Synapse` and never calls other handlers directly. The planned
entry point follows the house convention:

```rust
// Intended shape — not implemented.
pub async fn ignite(synapse: Synapse) -> anyhow::Result<()>;
```

`ignite` would subscribe to the `Synapse` and run an event loop that translates
guest-lifecycle requests (spawn, start, stop, snapshot, restore) into backend
operations, and reports guest status, console output, and resource usage back
onto the bus. The concrete `SMessage` variants for these requests and results
are not yet defined; adding them is a deliberate, reviewed change to the
`bandy` enum.

## Open design questions

- Backend selection: direct QEMU process management vs. `libvirt` bindings.
- The exact `SMessage` request/result variants and how guest console I/O is
  surfaced to the GUI layer (Quartzite).
- Container support scope (which runtime, image format) relative to full VMs.
- Host requirements: hardware virtualization (VT-x / AMD-V) and the required
  host packages must be detected and reported, not assumed.

## See also

- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
  — the userspace component model and the Bandy bus.
- [`docs/CODEX.md`](../../docs/CODEX.md) — the full handler manifest.
