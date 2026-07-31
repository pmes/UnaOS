# RELAY — start here, every session, before anything else

Two lanes. Each is a **fresh session** with its **own worktree** and **its own brief**.
Read your brief, work only in your tree, and put every artifact where the brief says.

| Lane | Your worktree | Your branch | Your brief |
|---|---|---|---|
| **kepler** | `~/src/github.com/pmes/UnaOS-gemini-kepler` | `wt/kepler-poke-x86` | [`video/Kepler/BRIEF-kepler-poke-terminal.md`](video/Kepler/BRIEF-kepler-poke-terminal.md) |
| **igpu** | `~/src/github.com/pmes/UnaOS-gemini-igpu` | `wt/gmux-igd-x86` | [`video/iGUI/BRIEF-igpu-gmux-igd.md`](video/iGUI/BRIEF-igpu-gmux-igd.md) |

Both trees are already created and already sit on trunk tip `ce5c6f49`. **Do not create
worktrees, do not rebase onto anything else, and do not touch the other lane's tree.**

---

## The five rules. These are why the last round was thrown away.

**1. COMMIT YOUR WORK.** The previous kepler session left its entire arc uncommitted in a
working tree while its HEAD still carried the defect the arc existed to remove. Had that
lane been merged on the strength of "the work is done", the dangerous version is what would
have landed. **Uncommitted work does not exist.** Commit every milestone; never report
complete against a dirty tree.

**2. YOUR ARTIFACTS LIVE IN THE REPO, NOT IN YOUR HEAD-SPACE.** Write your plan, your
walkthrough and your findings as files in your tree under `docs/dev/GEMINI/`, and **commit
them**. Exact paths are in your brief. Anything that lives only in a session-private
scratch directory is invisible to review and will be treated as not having happened.

**3. THE GATE MUST CARRY THE KNOBS THAT COMPILE YOUR CODE.** A green gate that did not
compile your change is not evidence — it is the single most expensive mistake in this
project's history and it has now happened three separate ways. Your brief names your exact
gate command. Run that one. Check the `⚡ kernel features:` banner actually lists your
feature before you believe any result.

**4. VERIFY WITH A TOOL, NOT WITH YOUR COMMENTS.** Four malformed falcon instructions
survived three review rounds because the assertions checked immediates and the comments
said the right thing. `envydis`, built from the in-repo `envytools`, is what caught them.
Where your brief names a verification tool, that tool's output is the evidence — not your
description of what the code does.

**5. STATE WHAT YOU DID NOT DO.** A commit message that claims a fix it does not contain
costs more than no commit at all. The last igpu commit was titled "Deferred GMUX IGD
Switch" and deferred nothing. If you ran out of road, say so in the commit body.

---

## Standing facts — do not re-derive these, they cost real boots

- **`./arroyo check` with no knobs compiles neither GPU driver nor the SMC.** Bare green is
  green about a build without your work in it.
- **A knob must be declared in THREE places** or it silently does nothing:
  `unaos/crates/kernel/Cargo.toml`, `unaos/arroyo`, and `unaos/builder/src/main.rs`. The
  builder rebuilds the kernel, so a knob missing there ships the feature disabled while
  every log line claims it is on.
- **Read serial captures with `awk`, never `grep`** — control bytes in the logs break grep.
- **The rig is Linux.** Everything runs on the Fedora host via
  `flatpak-spawn --host bash -c 'export PATH=$HOME/.cargo/bin:$PATH; …'`.
- **Metal is the verdict.** QEMU cannot reach these paths. Never run a QEMU suite.
- **Ask before you touch a file outside your lane.** `main.rs`, `shell.rs`, `arroyo`,
  `builder/` and `interrupts.rs` belong to the integrator. If your change needs one, say so
  and stop — a two-line seam is cheap to hand over and expensive to collide on.

## What the last round did get right — keep doing this

- Building `envydis` from the in-repo `envytools` and disassembling the extracted byte
  arrays. Zero unknown instructions, every branch displacement resolved by arithmetic.
  That method is now standing law for any microcode change.
- Whole-slice `const _: () = assert!` matchers that pin the destination register, not just
  the immediate.
- Deriving port immediates from `falcon_io()` rather than hand-writing them.
- The `RevertState` pack/unpack helper — one encode/decode point instead of open-coded
  shifts at fourteen sites.
- Diagnosing the `usbdebug` enclosure correctly, against the integrator's contradiction.
  That one was right and it is now fixed in trunk (`6e7b64d6`), credited.
