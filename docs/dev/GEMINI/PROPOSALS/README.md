# Gemini proposals — review-before-run

**Layout (from 2026-07-22):** new work files under
`../IMPLEMENTATIONS/<area>/<lane>/` — `video/iGUI/`, `video/Kepler/`,
`www/aether/`. Each lane directory holds its coordinator `BRIEF-*`, the
specialist's `PROPOSAL-*`, and the `REVIEW-*` notes. The workflow below is
unchanged; only the location moved. This directory keeps the pre-2026-07-22
Kepler pull records (pulls 4–6) as history.

Workflow (standing, from 2026-07-21):

1. Gemini writes its implementation plan for a pull HERE, as
   `PROPOSAL-<lane>-pull<N>.md` (e.g. `PROPOSAL-kepler-pull3.md`,
   `PROPOSAL-aether-pull3.md`), commits it to `UnaOS-gemini`, and pushes.
   **No implementation commits until the proposal is approved.**
2. First line of the file is a status header: `STATUS: PROPOSED`.
3. The reviewer (Claude session) reads it from git, and answers with either
   amendments (a `REVIEW-` note in this directory or relayed by Peter) or approval.
   On approval the header becomes `STATUS: APPROVED (<date>)` — edited by the
   reviewer or by Gemini quoting the approval.
4. Gemini implements per the approved text. Deviations discovered mid-pull go in the
   REPORT, not silently into code.
5. After the pull lands, the proposal stays here as the record of what was approved
   (do not delete; append `STATUS: LANDED <commit>` at the top).

Plans-of-record from the integrator side (PLAN-GEMINI-*.md) stay where they are
(`docs/dev/USERLAND/`, `docs/dev/OS/08_VIDEO/`); this directory is for Gemini's own
execution proposals against those plans.
