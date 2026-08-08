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
