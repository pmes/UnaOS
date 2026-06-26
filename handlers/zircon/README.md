# zircon — time and scheduling handler

Zircon is the UnaOS time-domain handler: it models calendars, schedules, and
project timelines, and surfaces milestones and deadlines as a timeline (Gantt)
view. It is one of the userspace handlers described in
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
and enumerated in the handler manifest in
[`docs/CODEX.md`](../../docs/CODEX.md).

## Status

**Design-stage (not yet implemented).** This crate currently contains only this
README. There is no `Cargo.toml` and no `src/`; no entry point, types, or bus
wiring exist yet. The sections below describe the intended design, not working
code.

## What it will do

Zircon is the scheduling and calendaring capability for a vessel. Its planned
responsibilities are:

- **Calendar / schedule** — maintain dated events, recurrence, and reminders as
  the canonical source of "what happens when."
- **Timeline (Gantt)** — render project tasks, milestones, and deadlines along a
  time axis so a workspace's schedule is visible at a glance.
- **Milestone tracking** — represent deadlines and milestones for the active
  project and signal when they approach or slip.

The earlier time-tracking framing (manual start/stop timers, Pomodoro focus
intervals, billing exports) is out of scope for this handler. Per the system
canon, Zircon's domain is **Time**: calendars, scheduling, and timeline views.

## How it plugs into the bus

Like every UnaOS handler, Zircon is a self-contained crate that communicates
only over the Bandy message bus — it does not call other handlers directly.
Following the handler convention, it will expose an async entry point (by
convention `ignite(...)`) that subscribes to the `Synapse` and reacts to
`SMessage`. The intended interface:

- **Consumes** — workspace/context changes (e.g. `Matrix(MatrixEvent)`) to learn
  which project is active and what tasks it contains; persistence results
  (`StorageQueryResult`) when loading stored schedules.
- **Emits** — UI updates and `StateInvalidated` so a subscribed GUI repaints the
  timeline; `StorageQuery` / `StorageSave` to read and persist schedule data
  through the storage handler.

Concrete `SMessage` variants for the time domain are not defined yet; adding bus
variants is a deliberate, reviewed change to `libs/bandy`.

## Relationships

- **Matrix** owns the workspace topology (files and tasks). Zircon is intended to
  read Matrix events so the timeline reflects the active project's milestones and
  deadlines.
- **Quartzite** (`libs/quartzite`) renders the timeline view natively and routes
  user input back as `SMessage`s; Zircon supplies the schedule state to display.

## Next steps

1. Add `handlers/zircon` to the workspace `Cargo.toml` and create the crate
   skeleton.
2. Define the time-domain `SMessage` variant(s) in `libs/bandy`.
3. Implement `ignite(synapse, ...)` with the subscribe/react event loop.
