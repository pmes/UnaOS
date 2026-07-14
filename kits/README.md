# kits

A **kit** is a *saved* composition — a snapshot of an elessar workspace, with
its handlers bound to views, captured as someone's helpful starting point.
Selecting a kit on UnaOS opens it live in elessar; on any other platform the
kit is compiled per-platform into a standalone **vessel** (see
[`vessels/`](../vessels)) for the try-without-install onramp.

Kits are the charter meaning of the old `apps/` directory. The reconciliation
that separated the three tempos of a composition — live (elessar workspace),
saved (kit), frozen-portable (vessel) — is recorded in
[`docs/dev/USERLAND/RECONCILIATION-2026-07.md`](../docs/dev/USERLAND/RECONCILIATION-2026-07.md);
the living model is
[`docs/dev/USERLAND/ARCHITECTURE.md`](../docs/dev/USERLAND/ARCHITECTURE.md).

This directory is a placeholder for that artifact class. The kit snapshot
format and the kit→vessel compiler do not exist yet — they are future arcs
(`aule` is expected to own packaging). Until then, the vessels under
[`vessels/`](../vessels) are hand-written prototypes of what that compiler must
one day produce.
