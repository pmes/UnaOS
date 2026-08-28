# The smart installer: AI-assisted metal bring-up through Vein

Status: DESIGN (orin, 2026-08-18). Charter: ROADMAP §1c rung **SH-4 smart install/debug**
("orin (design), all") — §1c currently lives on `hw-pi4`'s `docs/ROADMAP.md` (commits
`80daf328`, `5d84dc66`, `0b2c1180`) and reaches trunk at the next integration. Companion
storage design: [`orin-unafs-root.md`](orin-unafs-root.md) (SH-2/SH-3), which references
this document by name. In-kernel counterpart: [`installer_engine.md`](installer_engine.md).

## 1. Charter and scope

SH-4's charter, in the ROADMAP's terms: the installer consults an AI through Vein to
auto-diagnose metal bring-up on new hardware from the serial verdicts, so a machine UnaOS
has never seen can onboard itself. Two rulings from Peter (2026-08-17) bound the design
before any code:

- **Provider-agnostic.** A person sets up their own API — Claude or whatever AI they
  connect — and gets the whole experience. Vein owns the provider abstraction, Principia
  owns the settings surface, Holocron owns the credentials. **No provider is hardwired.**
- **Offline arm.** When the target machine cannot reach the internet, the smart half runs
  on UnaOS-on-host — the host-native userspace on `libs/gneiss_pal`, already running on
  macOS and Linux — as the connected companion driving the target's bring-up over the wire.

The eventual carrier is the **`UnaOS_Installer` vessel** in `vessels/`: one downloadable
program with two faces — (a) install UnaOS itself onto a machine or card; (b) install and
run individual UnaOS vessels on a foreign host. This document designs the smart half of
that vessel and specifies the *thinnest working loop* that this arc delivers (§5): a
`tools/`-resident CLI that reads a finished serial log, runs the existing witness-spec
verdicts, and takes one provider round-trip to a printed diagnosis. Flashing, wire
driving, and the vessel itself are later rungs, named here and not built.

Explicitly out of scope for this arc: any change to the in-kernel installer engine; any
change to `mbench.py` or the serial bridges; any provider credential handling beyond
reading what Holocron/the host environment already provides.

## 2. The provider abstraction

### 2.1 The problem being lifted

`libs/gneiss_pal`'s `ResilientClient` (`api` module) is today a **Google Vertex / Gemini**
client: `generate_content` / `embed_content` over the Vertex content API, credentials
fetched via `gcloud` ADC, a 401-refresh retry. `handlers/vein`'s brain loop constructs that
client directly and wraps it in `SynapticRetry` (`vein::synapse.rs`). That hardwiring is
precisely what SH-4 must lift — not by swapping one vendor for another, but by putting a
seam where the vendor currently is.

### 2.2 The seam

A **provider trait** at the `gneiss_pal`/Vein boundary. `ResilientClient` becomes one
implementation behind it rather than the type Vein names. The surface is the intersection
of what Vein actually uses today plus what a diagnosis loop needs:

| Operation | Purpose | Notes |
| --- | --- | --- |
| `request` | one prompt → one complete response | the diagnosis loop's only requirement |
| `stream` | incremental response | Lumen's chat experience; optional per provider |
| `embed` | text → vector | the Semantic Vault / engram path; optional per provider |
| `capabilities` | which of the above this provider supports, plus context limits | lets callers degrade instead of failing |

Design constraints on the trait:

- **Provider-neutral types.** The request/response types are UnaOS's own (roles, parts,
  attachments, usage), not a re-export of any vendor's wire schema. Each connector
  translates. `gneiss_pal::api`'s current `Content`/`Part`/`FileData`/`UsageMetadata`
  are the Vertex wire types and stay *inside* the Vertex connector.
- **Retry stays above the trait.** `SynapticRetry`'s backoff-with-jitter is provider-neutral
  and wraps any implementation. Connector-specific retries (Vertex's 401/ADC refresh) stay
  inside their connector.
- **Optional operations degrade honestly.** A connector that cannot embed reports so through
  `capabilities`; the caller skips the engram path rather than erroring at call time.
- **Errors are classified, not stringly-typed** — at minimum: auth, rate-limit/backoff,
  request-too-large, transport, provider-refused — so `SynapticRetry` and the diagnosis
  loop can decide without parsing prose.

### 2.3 Claude as the first connector, never the only socket

A Claude connector is the first implementation written against the trait, because it is the
one Peter runs. It is written as *a* connector, in the same shape any other connector would
take, and the trait is reviewed for neutrality by the test that a second connector (the
existing Vertex client, retrofitted) drops in without changing the trait.

**Model identifiers and request parameters are not recorded in this document and must not
be taken from memory.** At implementation time, model ids, parameter names, limits, and the
streaming/tool-use request shapes are read from the `claude-api` reference. Nothing about a
specific model generation belongs in a design doc that will outlive it.

### 2.4 Selection, configuration, credentials

Three handlers, three responsibilities, per the CODEX manifest (Vein = AI / provider
abstraction; Principia = system settings and policy; Holocron = secrets):

- **Principia** owns the settings surface: which provider is selected, which model, and the
  per-provider knobs. The installer and every other Vein consumer read the selection from
  Principia over the bus — they never carry a default vendor of their own.
- **Holocron** owns credentials. A connector asks Holocron for its credential by handle and
  receives it at call time; the credential is not persisted by Vein, not logged, and not
  written into any transcript (§3.4).
- **Vein** owns the abstraction and the connector registry: it maps Principia's selection to
  a connector instance and exposes only the trait upward.

Non-negotiables: **no provider identifier, endpoint, key, token, or account is committed to
the repository**, in code, in fixtures, or in docs. No connector is compiled in as a default
that activates without an explicit selection. A build with no provider configured is a valid
build: the diagnosis loop reports "no provider configured" and the verdict table still
prints — the deterministic half of the tool never depends on the AI half.

## 3. The auto-metal-debug loop

The smart half. This builds **on** the existing host-side bench machinery, not beside it:
`unaos/scripts/jetson-serial-bridge.py` (and its rmbp/pi siblings) own the serial port on
one never-reopened fd and write the capture log; `unaos/scripts/mbench.py` reads that log —
never the device — and asserts it against a checked-in witness spec
(`unaos/scripts/specs/*.spec`: `REQUIRE` / `COUNT` / `OPTIONAL` / `FORBID` / `PENDING`,
plus the always-on default `FORBID` set), printing a battery-style verdict table and
exiting 0/1. The bench runbooks (`unaos/scripts/*-bench.md`) carry the expected-vs-observed
narrative for each sitting. The loop composes exactly these three artifacts.

### 3.1 The cycle

1. **Boot** — bring the target up (attended power, or an automated arm on a later rung).
2. **Capture** — the bridge writes the serial log; the tool reads the log file only. The
   port-ownership rule is absolute and inherited unchanged from the bridges.
3. **Assert** — run the witness spec over the capture, producing the verdict table:
   which witnesses landed, which are missing, which forbidden lines matched, which
   `PENDING` witnesses fired (promotion candidates).
4. **Assemble a bounded diagnostic context** (§3.2).
5. **Consult** the configured provider through the Vein trait.
6. **Propose** a next action from a closed set (§3.3) with a stated rationale.
7. **Gate and iterate** — human approval on anything destructive; bounded iteration count.

### 3.2 The diagnostic context (bounded by construction)

Three parts, each with a hard budget so a runaway log can never blow the request:

- **The verdict table** — the full structured result: every directive, its regex, hit count,
  and pass/fail. This is small and is never truncated; it is the highest-signal artifact.
- **A tail of the capture** — the last N sanitized lines, plus the lines surrounding each
  `FORBID` hit and each last-landed `REQUIRE`. Sanitization reuses mbench's rule: read as
  bytes, decode UTF-8 with replacement, strip ANSI escapes and C0 control bytes (the reason
  plain `grep` is banned on these logs).
- **Expected-vs-observed** — the relevant excerpt of the bench runbook for this sitting:
  what the arc said should happen, against what the verdict table shows.

Budgets are configuration, not constants scattered in code, and the assembled context is
written to the transcript verbatim before it is sent — what the model saw is reconstructable.

### 3.3 The action set and the approval gate

The proposal is constrained to a small closed set, so the model chooses among actions the
tool can actually perform rather than emitting free-form instructions:

| Action | Destructive? | Gate |
| --- | --- | --- |
| re-run the same image, collect again | no | automatic |
| collect more (widen the spec, extend the timeout, enable a diagnostic knob) | no | automatic |
| re-flash with knob X changed | **yes** | human approval, required |
| power-cycle / DC-cut the target | **yes** | human approval, required |
| stop and report (insufficient signal, or diverged from the runbook) | no | automatic, terminal |

**Every destructive step — any flash, any power action — requires explicit human approval
at the moment it is proposed.** Approval is per-action, never blanket, never inferred from a
previous approval, and never granted by anything the model emitted. This is the same
discipline the in-kernel engine already runs on (the three-gate escalation ladder and the
about-to-destroy announcement in `installer_engine.md` §INSTALL-1/§INSTALL-PI); the host
side does not get a weaker rule than the kernel side. The flash-staging rule stands
unchanged: flash a staged, stamped, sha256'd tar, never a `target/` path.

The loop is **bounded**: a maximum iteration count and a maximum wall-clock, both
configured, both terminal. "Stop and report" is a first-class outcome, not a failure mode.

### 3.4 The transcript

Every iteration appends to a full transcript: the assembled context, the exact request
(minus credentials), the response, the parsed action, the approval decision and who made it,
and the resulting verdict table. The transcript is the artifact a bench sitting keeps.
Credentials, tokens, and any Holocron-supplied material are never written to it.

**A model's output is data, not authority.** The proposal is parsed into the closed action
set above and validated before anything runs; text in a serial log or a model response never
selects an action on its own and never elevates a gate.

## 4. The offline arm

When the target cannot reach the network — the normal case for a machine mid-bring-up, and
the Orin's situation while NET-4 is still being fought — the smart half runs on the **host**:
the same program, as UnaOS-on-host, in the host-native userspace layer (`libs/gneiss_pal` +
`libs/bandy` + the Quartzite host presentation) that Lumen already runs in on macOS and
Linux. The host has the internet; the target has the serial line; the host is the connected
companion.

The design consequence is a rule for day one: **the host tool is written in that layer from
the start** — `gneiss_pal` for host services, bus-wired like every other handler, Vein for
the provider — so online and offline are one codebase with one seam, not two programs that
drift. The online case (the installer running on the target, with its own network) and the
offline case (the companion on a Mac/Linux host, driving the target over the wire) differ
only in *where the serial capture comes from* and *which machine holds the credential*.
Both talk to the same provider trait; both run the same verdict/assemble/consult/propose
cycle.

This also lines up the two faces of the `UnaOS_Installer` vessel: the companion that
diagnoses a target's bring-up and the program that installs vessels onto a foreign host are
the same binary in the same layer.

## 5. The thinnest working loop (this arc's deliverable after design)

Per the naming law, a CLI lives in `tools/`; vessels come later. Working name:
**`tools/foreman`** — plain, descriptive, and **OPEN: flagged for Peter's naming pass**,
along with the vessel naming in §6. Nothing downstream should depend on the name.

Scope of the thin loop — deliberately one step wide:

- **Input**: a *finished* serial log (the bridge's capture, or a QEMU `target/serial*.log`)
  plus a spec path. No device is opened, no `--follow`, no injection.
- **Assert**: run the witness-spec verdicts over the log and print the same verdict table
  the bench already reads.
- **Assemble**: build the bounded context of §3.2.
- **Consult**: exactly **one** provider round-trip.
- **Output**: print the diagnosis and the proposed next action, and write the transcript.

Explicitly **not** in the thin loop: flashing, power control, driving the wire, injection,
multi-iteration, and any destructive action whatsoever. Those are the next rungs; the thin
loop cannot perform them, so its approval gate has nothing to gate — the gate arrives with
the first action that needs it, and not before.

### 5.1 Module seams (so the vessel can absorb it whole)

The CLI is a thin `main` over four modules that carry no CLI assumptions, so the
`UnaOS_Installer` vessel later links the same modules and supplies its own front end:

| Module | Responsibility | Boundary |
| --- | --- | --- |
| `capture` | read a log as bytes; sanitize (UTF-8-replace, strip ANSI + C0); yield lines with positions | knows nothing about specs or ports |
| `verdict` | parse a `.spec`; evaluate directives over sanitized lines; produce a **structured** result (not a printed table) plus a renderer for it | the one place the spec grammar is understood |
| `context` | assemble the bounded diagnostic context from a verdict result + capture + runbook excerpt, under explicit budgets | pure; no I/O, no provider |
| `advisor` | take a context, call the provider through the Vein trait, parse the response into the closed action set, return proposal + rationale | the only module that touches a provider |

`main` wires them and prints. The transcript writer is a sink the CLI passes in, so the
vessel can route transcripts to its own storage.

**Relationship to `mbench.py`.** `mbench.py` remains the bench's tool and its spec semantics
are the reference. The `verdict` module must agree with it directive-for-directive —
including the default `FORBID` set and the binary-safe sanitization — and the cheapest way
to keep that true is a shared corpus: the same log + spec pairs (including mbench's
`--self-test` canned lines) evaluated by both, asserted equal. Whether the Rust side
re-implements the grammar or shells out to `mbench.py` for the first cut is an
implementation call at build time; the *interface* the rest of the design depends on is the
structured verdict result, not the printed table.

## 6. Open calls (named, not acted)

- **Vein bus transport.** Properly, the CLI should reach Vein over the Bandy bus like every
  other component, with Vein resolving Principia's selection and Holocron's credential. The
  thin loop may instead link `gneiss_pal`'s client directly to get one round-trip working.
  **If it does, that is scaffolding and must be labelled as such in the code** — it is a
  direct-call shortcut past the bus, which the handler architecture does not permit
  permanently. Removing it is a named follow-up rung, not an optional cleanup.
- **`r8169` firmware load** (the NET-4A real-fix candidate) — **LICENSING, Peter's call**,
  unchanged. It gates the Orin's own-network (online) arm; the offline arm is specifically
  the design's answer to not waiting on it.
- **Vessel and tool naming** — `UnaOS_Installer` is the ROADMAP's working name for the
  vessel; `tools/foreman` is this document's working name for the CLI. Both are **OPEN for
  Peter's naming pass**.
- **Provider trait placement** — whether the trait lands in `gneiss_pal` (with connectors
  beside it) or in a Vein-owned crate that `gneiss_pal` implements against is a shared-crate
  decision touching files outside this track's lane; it is raised for the integrator rather
  than settled here.
