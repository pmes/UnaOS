# Userspace Reconciliation — July 2026

**Status: decided.** This document records the userspace-model reconciliation between the
Architect and the R14 Maestro (2026-07-14). It is the decision record; the living model is
[`ARCHITECTURE.md`](ARCHITECTURE.md) and the canon is [`docs/CODEX.md`](../../CODEX.md).
Directory moves and code corrections listed here are **adopted but not yet executed** —
each becomes its own arc.

## Why this document exists

A read-only survey (charter vs pre-drift docs vs disk vs current docs, per component)
confirmed that the 2026 bot-drift era (`google-labs-jules[bot]` sessions) had been largely
healed per-handler — amber_bytes restored (`80b7761`), matrix genesis re-widened, READMEs
returned to charter — but left one **structural** drift standing: `apps/` holds live source
vessels, no directory anywhere holds the charter's "elessar-workspace snapshots," and the
previous ARCHITECTURE.md §Vessels codified that drift as canon. Reconciling it forced the
whole userspace model to be stated precisely, which surfaced and settled the decisions below.

## The model (decided)

**A handler is headless.** It owns one capability area and has no path to the user. Vein can
think but has no mouth; junct can receive from every network but has nowhere to put a
message. Quartzite is the only path to the user, and a **view** is a specific path (the chat
view, the grid view, the viewport). Views are reusable base code by design: one chat view
serves both junct (human↔human) and vein (human↔AI).

**junct and vein are symmetric platform abstractions.** junct abstracts human conversation
networks (Matrix, Email, IRC, RSS → one Stream); vein abstracts AI providers (local/cloud →
one conversation). Same shape, same view, different party on the far end. Neither owns a
chat UI — that is the shared chat view's job.

**One thing at three tempos.** An **elessar workspace** is a *live* composition — handlers
bound to views, reshaping with context. A **kit** is a *saved* composition — a snapshot of an
elessar workspace, someone's helpful starting point. A **vessel** is a *frozen-portable*
composition — a kit compiled into a standalone binary that runs without elessar and without
UnaOS. Selecting a kit on UnaOS opens it live in elessar; on any other platform the compiled
vessel is the onramp ("try lumen on your Mac, no install"). Today's vessels are hand-written
because the kit→vessel compiler does not exist yet; they prototype what that compiler must
produce.

**A vessel ships the userspace kernel, not a parallel world.** A vessel runs gneiss (the
Ring 3 kernel) on the host: UnaOS gets inside the host and improves it, rather than blobbing
an entire foreign userspace on top with a native-looking skin. Light, native-light,
host-respecting — the opposite of the embedded-browser-runtime pattern.

**Elessar, defined.** Elessar is the **workspace runtime**: the thing that takes a
composition and makes it live, binding handlers to views and reshaping as context shifts. It
was never dropped — it was hard to define in isolation, and is only definable in terms of
kits and vessels (above). The 148-LOC context-detection crate on disk is the honest seed:
detection is step one of binding.

**Safety: law → authority → interlock.** Three layers, each already or newly chartered:
- **principia** (The Architect / policy engine) *states* the safety levels — user-chosen,
  per action-domain, from "never" through "ask" to "autonomous within bounds" — for anything
  an AI will drive: actuators first, other consequence domains as they arrive.
- **helm** (new handler — see the CODEX amendment) *holds control authority*: every
  AI-initiated physical action passes through helm, which reads principia's levels and
  decides pass/ask/refuse. The helm is the wheel and the captain's voice: direct human
  control and commanded intent at one station, one authority deciding which is in effect.
- **the kernel helm core** (`unaos/libs/sys/helm/`) is the hard interlock beneath —
  DISARM/MANUAL/AUTO with a FAILSAFE latch, transmitter-as-human-estop — the layer that does
  not negotiate. Per-machine domains (`src/rover/` first) because failsafes do not
  generalize: a rover's safe state is "stop"; a mill's is "retract and stop the spindle."

**comscan is purely the wire.** Transport in (telemetry → squawk), transport out
(helm-approved commands → actuators). **squawk** is the telemetry capability *within*
comscan — all telemetry, from any hardware, hands off through it.

**The user surface is anarchy, not total chaos.** No imposed hierarchy — handlers surface
through views in the spatial interface wherever the user arranges them; order emerges from
composition, not from a launcher-menu bureaucracy. principia is the law everyone reads;
helm is the one place authority over consequences lives. This is why there is no app grid.

## The directory map (adopted; migration arcs pending)

| Location | Contents |
| --- | --- |
| `handlers/<name>/` | The handlers, **flat** — each of the (now 21) locked names is a top-level subdir; the name *is* the charter. Internal structure is per-handler (e.g. `comscan/src/transport/{serial,gpio,bt,sdr}/` + `comscan/src/caps/squawk/`). |
| `vessels/` | Source vessels (lumen, facet, phonolite, pulse, una) — moved out of `apps/`. |
| `kits/` | Elessar-workspace snapshots (the charter meaning of the old `apps/`): users' starting points, compiled per platform as vessels for the try-without-install onramp. |
| `tools/` | The command-line tools (`unafs`, `vertex`, `sentinel`, `unafs_bench`) — the old `apps/cli/*`. |
| `libs/` | Host-native userspace libraries (gneiss_pal, bandy, quartzite, euclase, resonance, lux, elessar). |
| `libs/views/` | Reusable view crates (the chat view first) — views depend on quartzite but are not part of it. |
| `unaos/libs/` | Ring 0-embeddable cores (`no_std`, kernel pulls with `default-features = false`), in device-class subdirs: `fs/` (the UnaFS format core both rings share), `input/` (ibus), `pwm/`, and `sys/helm/` (system authority, not a device class). |

**UnaFS divides** along the ring seam: the `no_std` format core (structures, checksums,
journal logic — no I/O) lives at `unaos/libs/fs/`, consumed directly by the kernel adapter
and wrapped with host I/O by the userspace `libs/fs/unafs`.

## Naming decisions

- **"seat" is retired.** The orchestrator session is the **Maestro**; agents are named by
  role: **executor** (owns an arc), **lens** (focused reviewer), **scout** (read-only
  evidence).
- **"vessel" stays** as the technical term (correct beats comfortable); users never see it —
  they download "lumen for macOS."
- **"kit"** replaces the drifted sense of "app" for the snapshot artifacts.
- **"drive" is dissolved.** It was doubly broken (disk-drive collision; the generalized
  "things AI will drive"). The crate's content *is* helm's kernel core; the rover state
  machine lives at `unaos/libs/sys/helm/src/rover/`.
- **"squawk"** stands as comscan's telemetry capability (not a vessel, not a handler).

## Corrections list (each an arc; none executed yet)

1. **`apps/` split** → `vessels/` + `kits/` + `tools/` per the map above. **Executed**
   (`us-apps-split`): the vessels moved to `vessels/`, the CLI tools to `tools/`, `kits/`
   scaffolded with a charter README, and all workspace members, path dependencies, and doc
   references updated.
2. **junct purge** — the `cpal` + `resonance` audio code in `handlers/junct` is confirmed
   bot hallucination (The Receiver was never implemented; a comms handler has no business
   with microphone deps). Archaeology (who/when), then delete; junct returns to a clean
   design-stage stub. **Executed** (`us-junct-purge`): archaeology confirmed the audio path
   was introduced by a `google-labs-jules[bot]` session (commit `64bace4`, 2026-02-19);
   the `cpal`/`resonance`/`tokio` deps and the FFT stream code are deleted, `lib.rs` is a
   charter-accurate design-stage stub, and the README rewritten to the "Receiver" charter.
3. **`libs/drive` → `unaos/libs/sys/helm/src/rover/`**, with `libs/input`/`libs/pwm`
   documented as the TALUS kernel-lane device-class libs (they are deliberate work, not
   drift — the survey initially flagged them only because they lack READMEs; READMEs owed).
   **Executed** — the crate is now `helm` (module `rover`) at `unaos/libs/sys/helm`;
   `libs/input` and `libs/pwm` carry READMEs; the helm core has a charter README.
4. **UnaFS ring-split** per the map. **Executed** (`us-unafs-ringsplit`): the crate
   was already `no_std`-capable as a single crate (the kernel consumes it with
   `default-features = false`; the host-native `FileDevice`/mmap/bandy surface sits
   behind the default-on `std` feature), so the ring-split is a **move**, not a
   two-crate fork — `libs/fs/unafs` → `unaos/libs/fs/unafs`, following the helm
   template (root-workspace member, `unaos/Cargo.toml` still `exclude = ["libs"]`).
   All workspace members, path dependencies (kernel, tools, handlers, vessels), and
   doc references updated; the crate's own sibling path deps rebased to `../../../../libs`.
5. **`libs/views/` extraction** — lift the chat view out of lumen as the first shared view
   crate.
6. **Handler README pass** — xenolith drops the "containers" scope-bleed (containers are
   geode's, VMs are xenolith's, per CODEX); una's lore-voiced README rewritten professional.
   **Executed** (`us-handler-readmes`): `handlers/xenolith/README.md` rewritten to the
   VMs-only charter (The Bridge) with an explicit geode-owns-containers scope note; the
   `vessels/una/README.md` IDE-vessel README rewritten from the lore register into the
   professional voice, keeping its substance and stating plainly that it is the `vessels/una`
   vessel, not a handler.
7. **helm handler scaffold** — `handlers/helm/` design-stage README against the CODEX
   amendment; principia README gains the safety-levels surface. **Executed**
   (`us-handler-readmes`): `handlers/helm/` scaffolded on the junct design-stage convention
   (charter README + stub `Cargo.toml` (`helm-handler`, no deps, standalone — not a
   workspace member) + `src/lib.rs` doc-comment stub), distinguishing the Ring 3 authority
   handler from the Ring 0 `unaos/libs/sys/helm` interlock core; `handlers/principia/README.md`
   gains the safety-levels surface (user-chosen, per action-domain, "never" → "ask" →
   "autonomous within bounds") under the law → authority → interlock framing.

## Still open (deliberately)

- **vaire scope** (The Loom vs the broader Bolt-managing workspace manager) — under the
  Architect's active revision; untouched here.
- **stria's video half** — deferred by design (the audio-only slice was the Architect's own
  first slice), not drift.
- The kit→vessel compiler and the elessar snapshot format — future arcs; `aule` is expected
  to own packaging.
