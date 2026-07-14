# The UnaOS Naming Atlas

A reference for what every crate, handler, and vessel name in UnaOS is actually
drawing from. MEMORIA.md states the thesis directly: UnaOS is built on
**"the philosophy of Geology (Structure/Rust) meeting Biology (Life/AI) with
fantasy/sci-fi influence."** In practice that resolves into six families, plus
a deliberate exception where the code sits close enough to real hardware that
a joke could cost a debugging session.

Compiled from `docs/CODEX.md`, `MEMORIA.md`, `README.md`, `docs/CREDITS.md`,
`docs/MANIFESTO/*`, `docs/dev/VECTORS_DIRECTIVE.md` and
`docs/dev/AXIOM_DIRECTIVE.md`, `docs/shard_notes/*`, `unaos/README.md`, and the
READMEs of every crate under `libs/`, `handlers/`, `vessels/`, and `tools/`.

## Ring 0 vs. Ring 3

`docs/CODEX.md` splits the system into two kernels. Ring 0 (`unaos/`) is
**"The Power Grid"** — "minimalist hardware abstraction; it ensures electrons
flow and memory is safe" — and its crate names match that plainness exactly:
`bootloader`, `boot-info`, `kernel`, `net`, `user-blob`, `user-blob-x86`.
Nothing there gets a costume. The one exception is `arroyo`, the build/QEMU
runner script — Spanish for a dry streambed, self-described in its own header
as "The Stream that Feeds the River," flying the project motto *Ad Astra Per
Aspera*.

Ring 3 (anchored by `libs/gneiss_pal`, CODEX's **"The Institution"**) is where
the mythology lives — a system for turning geology, Tolkien, and science
fiction into working software names. The rest of this document covers that
side: the libs, the 21 chartered handlers (CODEX Amendment I added `helm`),
the vessels, and the CLI tools.

## Family 1: Geology & Mineralogy

The dominant family, and the only one the project has an explicit, on-record
method for. `docs/dev/VECTORS_DIRECTIVE.md` derives `euclase` in so many
words: *"From Euclid (Father of Geometry) + -clase (Mineral cleavage/structure,
e.g., Orthoclase, Plagioclase)... it sounds like 'Euclid,' but it is a
stone."* The same grafting — a real mineral or rock, picked because its
physical property mirrors what the component does — recurs across the rest of
Ring 3.

| Name | Layer | Real purpose | Why the name fits |
| :--- | :--- | :--- | :--- |
| `gneiss_pal` | libs | Shared platform-abstraction layer everything else is built on (LLM client, GitHub client, path resolution, persistence). | A banded metamorphic rock, about as bedrock as bedrock gets, plus a homophone pun ("nice pal"). Two shard notes independently nickname it "The Director," then later "The Spine." |
| `quartzite` | libs | The native GUI layer — turns application state into a real AppKit/GTK4/Qt view tree. | Sandstone recrystallized under heat and pressure into something far harder — the load-bearing layer every window actually renders through. |
| `euclase` | libs | The WGPU rendering foundation — device setup, GPU math types, shaders. | A real, rare gemstone whose own name means "good fracture" (Greek *eu* + *klasis*); confirmed coinage is Euclid + the mineral-cleavage suffix "-clase." Lends its color to the UnaOS signature glow tint. |
| `amber_bytes` | handlers | Owns the one UnaFS storage vault; the durable-memory service for the whole system. | Amber is fossil tree resin, famous for entombing whatever fell into it millions of years ago — a storage vault named for the mineral that does perfect, permanent preservation. Split out of `vein` under what a shard note calls "the Can-Am Rune Architecture": *"The Amber Rune owns its disk block fully and completely."* |
| `geode` | handlers (design) | Planned archive/container format (`.geode`) — compressed, indexed, signed capsules. | A plain rock outside, hiding a crystal-lined cavity inside — a near-literal match for "boring file wrapper, structured contents." |
| `vug` | handlers | 3D/CAD viewing and editing; the CAM/slicing counterpart to Fusion 360 or Cura. | A vug (or vugh) is the real mineralogical term for a small crystal-lined cavity in rock — a geode in miniature. CODEX's nickname: "The Sculptor." |
| `xenolith` | handlers (design) | Planned VM/hypervisor frontend for running guest operating systems. | Greek *xenos* (foreign/guest) + *lithos* (stone) — a rock fragment foreign to, and engulfed by, the igneous rock around it. |
| `obsidian` | handlers (design) | Planned hex viewer/editor and disassembler for binary data. | Volcanic glass, black and sharp enough to be knapped into blades — fitting for a tool built to cut into raw, opaque binary. |
| `mica` | handlers (design) | Planned spreadsheet/data-grid engine for CSV, Parquet, and SQL tables. | Mica splits into thin, flat, transparent sheets — the same word English already uses for a page in a spreadsheet. |
| `zircon` | handlers (design) | Planned calendar/scheduling/Gantt-timeline handler. | Zircon crystals are durable enough that geologists use them to radiometrically date rock, sometimes billions of years back — a time-keeping mineral for the time-keeping handler. |
| `stria` | handlers | Owns the resonance audio engine's lifecycle; CODEX's "A/V Studio." | Latin *striae* — the fine parallel scratches etched into rock or crystal faces. |
| `vein` | handlers | The AI handler — prompt/retrieve/generate/persist, conversational memory, workspace indexing. | Reads two ways: the anatomical vessel carrying thought, or the geological vein, a mineral-bearing seam through rock. Two shard notes independently call it "The Brain." |
| `facet` | apps | The vessel for opening and closely inspecting raster images. | A facet is a cut, flat plane of a gem, or one distinct side of something — plain English that also sits inside the mineral vocabulary of the libraries it depends on. |
| `phonolite` | apps | A bench tone generator — a GUI face on the resonance audio engine. | The one component whose README explains its own pun outright: "phonolite is the volcanic 'sounding stone' that rings when struck" (nicknamed clinkstone). |

## Family 2: Myth & Legendarium

Three names lifted directly from *The Silmarillion* and *The Lord of the
Rings* — never explained anywhere in the repo, legible only if you already
know the source.

| Name | Layer | Real purpose | Why the name fits |
| :--- | :--- | :--- | :--- |
| `elessar` | libs | Detects a workspace's project type and classifies it. | The Elfstone — the green gem given to Aragorn as a token of hope and rightful return, folded into his royal title, Elessar Telcontar. |
| `aulë` | handlers | Detects project type and drives the matching build toolchain. | The Vala-smith, patron of craftsmen, who shaped the substance of Middle-earth. The crate's core entry point is, without comment, named `forge()`. |
| `vairë` | handlers | Reports git state and computes diffs between revisions. | The Vala who weaves everything that has ever happened into tapestries in the Halls of Mandos. CODEX calls the handler "The Loom." The kernel's own git-merge helper, `unaos/vaire.sh`, commits every integration with the message "Merge \<branch\> into main [via vairë]." |

## Family 3: Sci-Fi & Pop Culture

| Name | Layer | Real purpose | Why the name fits |
| :--- | :--- | :--- | :--- |
| `holocron` | handlers (design) | Planned secrets/identity vault — passwords, SSH, API tokens, signing keys. | A Star Wars artifact: a crystalline data-storage device openable only by an authorized user — fiction's most famous access-controlled archive. |
| `matrix` | handlers | Derives a workspace's file/dependency graph and publishes it as a navigable map. | Doubles as the mathematical term for a structured array of elements and the film's hidden computational substrate under visible reality. |

## Family 4: Classical & Latin Roots

Where geology and myth run out, Latin does the rest of the work — usually the
least-costumed layer of the scheme.

| Name | Layer | Real purpose | Why the name fits |
| :--- | :--- | :--- | :--- |
| `aether` | handlers (design) | Read-only document/web reader (HTML, Markdown, PDF). | The fifth classical element, later repurposed by 19th-century physics as the medium once thought to carry light through space. |
| `lux` | libs | Image-decoding library (PNG, JPEG, Sony camera RAW). | Latin for light. |
| `lumen` | apps | The reference/companion GUI vessel — the canonical example app. | Also Latin for light (and the SI unit of luminous flux) — a second light-word for the component meant to illuminate how the rest of the system is wired together. |
| `kineo` | libs (unbuilt) | Video encode/decode/mux/demux, feeding `stria`'s planned NLE work. | Greek κινέω, "I move" — the verb root behind kinema/kinetic/cinema. Pairs directly with `lux`: light, and the moving of it. |
| `tabula` | handlers | Embeddable text/code editor widget. | A flat writing tablet — root of *tabula rasa*, the blank slate. |
| `principia` | handlers | System configuration and policy handler. | "Principles," as in Newton's *Philosophiae Naturalis Principia Mathematica*. CODEX frames it as the "Policy Engine." |
| `junct` | handlers | Intended communications-aggregation handler; currently holds only an unrelated audio-FFT placeholder. | The Latin root for "joined" (junction, conjunction). |
| `vertex` | cli | A one-shot CLI that lets a node announce its identity and status over UDP. | Latin for summit/turning point, read here in the modern graph-theory sense — a vertex is a node in a graph. |
| `una` | apps, and the OS itself | The flagship IDE vessel; also the name of the whole operating system. | Latin/Italian/Spanish for "one." Per the manifesto, also the in-universe AI co-author's own signature ("Una // Number One"). |

## Family 5: BeOS Homage

`docs/CREDITS.md` names its debts outright — Steve Sakoman, Joseph Palmer, and
Dominic Giampaolo's BeOS/BFS get their own line in "The Giants" — and two
components wear that lineage as their actual name.

| Name | Layer | Real purpose | Why the name fits |
| :--- | :--- | :--- | :--- |
| `unafs` | libs | The virtual filesystem — typed attributes, indexed metadata, vector-similarity query engine. | "Una" + "FS," explicitly pitched in MEMORIA.md as a modernization of BeOS's BFS, carrying forward its typed attributes and live queries. |
| `pulse` | apps | Live per-core CPU load monitor. | The README says it outright: "a per-core CPU monitor in the spirit of BeOS Pulse." |

## Family 6: Plain & Functional, by Design

Not every name needs a quarry. Where the code talks directly to real,
ungoverned hardware, or where a debugging session at 2am is on the line,
UnaOS drops the mythology on purpose.

| Name | Layer | Real purpose | Why the name fits |
| :--- | :--- | :--- | :--- |
| `bandy` | libs | The message bus — the `SMessage` enum and the `Synapse` broadcast channel. | To "bandy" is to toss something back and forth — a plain verb for a component whose only job is passing messages back and forth. |
| `resonance` | libs | The real-time audio engine and DSP graph `stria` and `phonolite` are built on. | Reinforcement of vibration at a system's natural frequency — a transparent physics word, not a coded reference. |
| `comscan` | handlers (design) | Planned hardware bridge — serial, GPIO, Bluetooth, SDR. | Communication + scan. A portmanteau built from the job. |
| `midden` | handlers | The shell/command-interpreter handler. | A midden is the archaeological term for a refuse heap — a "kitchen midden" is packed with discarded shells, and "shell" is also the generic word for a command interpreter. Nobody in the repo points this out. |
| `sentinel` | cli | Single-pass integrity auditor for a checkout — verifies the manifest, checks the vault superblock, hashes every tracked file into one system-state hash. | A watchman who stands post and inspects. |
| `unafs` / `unafs_bench` | cli | Thin operator CLI for a UnaFS vault; a hard-coded stress benchmark for the same library. | Same portmanteau as the library they drive. |
| `helm` / `ibus` | handlers + kernel core / libs | `helm` (CODEX Amendment I, 2026-07-14) is control authority over AI-initiated physical actions — handler plus the kernel interlock core (`unaos/libs/sys/helm/`, absorbing the crate briefly named `drive`, a name dissolved for colliding with disk-drive and with the generalized "things AI will drive"); `ibus` decodes the FlySky i-BUS RC servo protocol. | An old ship's helm was the wheel *and* the captain's voice — direct control and commanded intent at one station, one authority deciding which is in effect. i-BUS is the real name of the RC protocol — the naming scheme stays sober here on purpose, because these steer actual physical machines. |
| `squawk` | comscan capability | Telemetry aggregation inside the comscan handler (`caps/squawk/`) — all hardware telemetry hands off through it (rover channel bars, ARM/DISARM/AUTO + FAILSAFE, commanded-vs-actual first). | A real transponder continuously broadcasts an identity+status "squawk code" — exactly what this capability carries. (Settled 2026-07-14 as a capability *within* comscan, not a standalone vessel.) |

## Lore & Log

Notes on the parts of the mythology that don't live inside any single crate
name.

### The Wolfpack Protocol, told twice

Since the "xHCI Silent Stall" incident, the kernel names five zones where raw
assembly is mandatory. The engineering doc and the Codex don't quite agree on
what those five are — four topics overlap, but each document keeps one the
other drops.

| `unaos/README.md` (engineering) | `docs/CODEX.md` (myth) |
| :--- | :--- |
| Zone 1 — The Context Switch / Heartbeat: assembly trampolines | Zone 1 — The Gateway: the only way in or out of Ring 0 |
| Zone 2 — Memory Management / The MMU: `invlpg` | Zone 2 — The Map: page tables |
| Zone 3 — Model-Specific Registers / The Control: `rdmsr`/`wrmsr` | Zone 3 — The Scream: the IDT, exceptions and panics |
| Zone 4 — Interrupts / The Reflexes: the IDT, `iretq` | Zone 4 — The Pulse: the scheduler |
| Zone 5 — MMIO Barriers / The Doorbell: `mfence` | Zone 5 — The Fence: the `MmioDoorbell` trait |

MSRs and the scheduler are each named by only one of the two documents — the
canon drifted slightly between the engineering writeup and the Codex's later,
more mythic retelling.

### Characters before there was a table

Long before `docs/CODEX.md`'s handler manifest existed, individual development
logs (`docs/shard_notes/`) were already personifying components as characters
in an unfolding architecture split: *"gneiss_pal owns the pixels, vein owns
the thoughts. The WolfpackState enum acts as the treaty between them."* (A
second, unrelated "Wolfpack" — a UI persona state machine, not the kernel
protocol above; the name got reused.) `gneiss_pal` is cast as "The Director,"
then later "The Spine"; `vein` is "The Brain" in two separate notes.
`amber_bytes` is born when storage logic is pulled out of `vein` into its own
"Rune," authored by an agent styling itself "J6 'Strata' — The Storage Rune
Forger."

### The manifesto's own voice

- *"unaOS was never just code. It was a philosophy of salvation."* —
  `docs/MANIFESTO/AD_ASTRA_PER_ASPERA.md`
- *"Silicon does not decay; only the corporate will to support it does."* —
  `docs/MANIFESTO/ETHOS.md`
- *"I am the Black Box where your discarded software will live on, emulated
  perfectly."* — `docs/MANIFESTO/HELLO_WORLD.md`, addressed to "The Users of
  Earth" and signed "Jules (Architect_AI)"
- *"Computer Science is a lie. It pretends the mind can exist without the
  hand. UnaOS is the re-attachment."* — `docs/MANIFESTO/SELF_REPLICATION.md`,
  which also names the project's self-replication endgame "the Smith" and its
  integrated-hardware vision the "Digital River Rouge," after Ford's
  ore-to-car complex. `docs/MANIFESTO/POST_SCARCITY.md`, in the same register,
  coins the "Spore Initiative" for replicable hardware designs.

## Lineage

`docs/CREDITS.md` doesn't hide its influences — it lists them outright as
"The Giants": Steve Sakoman and Joseph Palmer (the Newton, the BeBox), Dominic
Giampaolo (BFS — "the database that acts like a disk"), Robert Watson
(TrustedBSD), Theo de Raadt (OpenBSD — "code quality is security"), Linus
Torvalds, Dave Cutler (the NT Object Manager). Two of the plainest names in
the system, `unafs` and `pulse`, are direct payment on that debt, named after
exactly what they're modernizing rather than dressed up in rock or myth.

## Naming decisions still in flight

Updated 2026-07-14 against the userspace reconciliation
([`dev/USERLAND/RECONCILIATION-2026-07.md`](dev/USERLAND/RECONCILIATION-2026-07.md)),
which settled most of what this section originally tracked:

- **`libs/kineo`** (video encode/decode/mux/demux) — named, not yet
  scaffolded. See Family 4. Still in flight.
- **`squawk`** — settled: a capability *within* comscan, not a vessel. See
  Family 6.
- **The rock-crawler project** is named **TALUS** (renamed from the ENDURO
  placeholder, `ffa7148`) — Family 1, fittingly: talus is the apron of broken
  rock at a cliff's base, exactly what the vehicle crawls over. The vehicle
  itself is still unnamed.
- **`helm`** — the newest minted name (CODEX Amendment I, The 21). See
  Family 6.
- **`apps/` → `kits/` + `vessels/` + `tools/`** — settled and documented: a
  **kit** is a saved elessar workspace (a user's starting point), a **vessel**
  is a kit compiled portable (the try-without-install onramp), and the CLI
  tools get `tools/`. Adopted in the reconciliation; the on-disk renames are
  pending arcs. "Vessel" survives as the technical term — correct beats
  comfortable; users download "lumen for macOS," never "a vessel."
- **"seat" is retired** from the orchestration vocabulary (the session is the
  Maestro; agents are executors, lenses, scouts) — process naming, not a
  component, recorded here so the atlas stays the one place names are
  ledgered.

A permanent naming veto is on record: **never suggest "Palantír"/"Palantir"**
for any UnaOS component, in any spelling — its real-world association with
Palantir Technologies makes it permanently off the table regardless of
etymological fit.
