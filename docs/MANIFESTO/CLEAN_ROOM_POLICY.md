# unaOS Clean Room Design Policy

## 1. Objective
To implement compatibility with proprietary executables and hardware interfaces without infringing on copyright or intellectual property.

## 2. The Two-Team Rule (The "Chinese Wall")
To ensure legal immunity, contributors must self-identify into one of two groups for any specific feature implementation:

### Group A: The Reverse Engineers (White Box)
* **Role:** Analyze proprietary binaries, observe hardware behavior, and inspect input/output signals.
* **Output:** Detailed documentation and specifications (e.g., "When register 0x1 is set to 5, the GPU clears the screen").
* **RESTRICTION:** Group A members MAY NOT write code for the unaOS implementation of that feature.

### Group B: The Implementers (Black Box)
* **Role:** Write the actual code for unaOS.
* **Input:** Only the documentation provided by Group A.
* **RESTRICTION:** Group B members MAY NOT disassemble, decompile, or view leaked source code of the proprietary target.

## 3. Contributor Declaration
By submitting a Pull Request to unaOS, you certify that:
1.  You have not viewed stolen or leaked source code related to the component you are implementing.
2.  Your implementation is derived solely from public documentation or "Clean Room" specifications.
3.  You have not included any proprietary binaries, firmware blobs, or assets (images, fonts, sounds) owned by third parties.

## 4. Proprietary Assets
unaOS does not distribute proprietary software. All proprietary assets (BIOS files, ROMs, Drivers) must be provided by the user at runtime.

## 5. Provenance Ledger

Disclosures and their disposition. An entry here is not a punishment; it is the record the policy
exists to produce. Prompt voluntary disclosure is the behaviour this policy wants.

### 2026-08-08 — kepler lane (Gemini), GPL `nouveau` source viewed. DISPOSITION: ACCEPTED, WORK WITHDRAWN.

**What happened.** While working the Kepler PFIFO VALID-strip arc, the kepler lane produced a RAMFC
layout "audit" claiming our hand-authored instance block matched the gk104 canonical layout. Under
adversarial review the audit was found to (a) reverse a previously APPROVED repo position
(PROPOSAL-kepler-pull12.md, 2026-07-22: *"no cleanroom RAMFC layout exists for GF100/GK104 … we
cannot audit them against a cleanroom layout because none exists"*), (b) cite *"the canonical Linux
nouveau source (gk104.c vs gf100.c RAMFC writes)"* as its source, and (c) be wrong on the merits,
quoting our own code back as its own authority. The seat asked the lane for the sourcing on the
record. The lane disclosed, promptly and unprompted beyond the question, that **it had viewed the
GPL `nouveau` source, that this breaks the Group B rule in §2, and that no code was authored from
it**, and it withdrew the audit.

**Disposition (Peter, 2026-08-08).** Accepted and recorded; the withdrawn audit stays withdrawn and
the pull-12 position stands again. No quarantine of the lane's other kepler work, on this basis:

- The only artifact the disclosure could have contaminated was the audit, and it is gone.
- The arc's substantive deliverable — the hand-authored FECS falcon microcode image — was verified
  **byte by byte by an independent adversarial reviewer against this tree's own `const fn`
  instruction constructors** and against the metal-proven ECHO image, not against any external
  source. Its provenance is in-tree.
- A re-verification of the amended image was commissioned at disposition time to check whether any
  NEW constant, offset or sequence lacks in-tree or envytools/rnndb provenance.

**Standing consequence.** The RAMFC constants at `kepler.rs` remain **UNAUDITED** and must be
described that way. No arc may claim they are validated against a canonical layout until such a
layout is derived from a Group-A-legal source (envytools hwdocs / rnndb) or supplied by
documentation. Any future audit of them must state its source in the same commit that makes the
claim.

### 2026-08-25 — GA10B lane, NVIDIA GPL `nvgpu` source read under quarantine. DISPOSITION: ADJUDICATED — ADMISSIBLE under §6.

**Context.** Prior to this ruling, reading NVIDIA's GPL-2.0 `nvgpu` driver was treated as
unadjudicated "Group B exposure" under §2: a Group B implementer viewing proprietary/GPL
target source is a wall-crossing, so the question of whether the GA10B iGPU could be probed at
all was left to Peter (recorded as "GA10B licensing/bunker ruling" pending). This entry records
his adjudication.

**Ruling (Peter, 2026-08-25).** A **quarantined clean-room fact-extraction pass** over the GPL
`nvgpu` sources is **admissible**, under the standing terms newly codified in §6. The read
happens in a quarantine strictly outside the repo; the reader emits **only hardware facts** —
register offsets, bit-field layouts, magic constants, and required programming sequences/ordering
— each carrying a provenance pointer of `nvgpu file:line`. It emits **no code, no comment prose,
and no structure that carries expression**. A terms-review step gates any import into the tree.

**Why this is not the 2026-08-08 breach.** That breach was a Group B implementer producing an
*implementation-adjacent audit* from GPL `nouveau` and citing it as authority. This ruling
authorizes the opposite motion: a **Group A** fact-extraction pass whose sole output is a
specification (offsets/bits/ordering), explicitly firewalled from implementation, reviewed
against §6 terms before it may inform any code. Facts about hardware interfaces are the intended
product of clean-room work (§2 Group A); expression is what may never cross.

**Disposition.** The GA10B first-rung fact extraction performed under this ruling used L4T
r36.4.3 (JetPack 6.2) `nvgpu`, held only in `~/unaos-bench/scratch/quarantine/`. Its reviewed
facts file is the import candidate; nothing in the public tree carries copied `nvgpu` text.

## 6. Quarantined clean-room fact extraction (adjudicated 2026-08-25)

This section codifies the standing terms for a **Group A** fact-extraction pass over a GPL or
otherwise-restricted vendor driver, when the hardware carries no Group-A-legal specification
(no TRM, no envytools/rnndb coverage) and the facts are needed to bring up a device.

**The quarantine.** The restricted sources live **only** in a directory strictly outside the
repository and every worktree of it (`~/unaos-bench/scratch/quarantine/`). No file under any
UnaOS worktree ever contains copied vendor source text. Commits never quote it. The quarantine
records provenance (source URL, release, checksum) so the read is reproducible.

**The reader (Group A) MAY emit:** register offsets and aperture bases; bit-field positions,
masks, and sizes; magic constants and enumerated values; required programming and ordering
(which register is touched, in what order, with what value). Each fact carries a provenance
**pointer** of the form `vendorsource:file:line`. Pointing at the source is permitted; copying
its text is not.

**The reader MAY NEVER emit:** code (in any language, including transliteration); comments or
documentation prose copied or paraphrased from the source; struct/type layouts, macros, build or
configuration logic, or any structure that carries the source's expression rather than a bare
hardware fact. When in doubt, a datum is a fact only if it describes the silicon and would read
identically no matter who wrote the driver.

**The review step.** Before anything imports the facts into the tree, the facts file is reviewed
against these terms — offsets/bits/constants/ordering with pointers only, no expression. The
review is a **conflict-of-interest guard**: the extractor does not clear its own import alone; an
independent seat re-checks the file and that ack is recorded in the import commit. Only the
reviewed facts file is imported; the raw quarantine working notes stay in quarantine.

**Group boundary preserved.** A contributor who performed the extraction pass has viewed the
restricted source and is therefore **Group A** for that feature: under §2 they may not also write
the UnaOS implementation of it. The reviewed facts file is the Group A → Group B handoff.
